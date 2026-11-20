// THE `clickup-sync` RUN, on the durable runtime (talaria-runs-define): one
// board's pull from its ClickUp list, as steps over a checkpoint.
//
// PASSES, in order:
//   probe        GET /user, then GET /list. A refused token stops the run here,
//                before anything could read "every task is gone" off a dead
//                credential; the list's path and statuses are recorded.
//   incremental  tasks updated since the watermark (minus two minutes of
//                slack), oldest first, a page at a time — each upserted.
//   archived     the same, archived=true: marks tickets "archived in ClickUp".
//   sweep        (first sync's next run, every Sync now, at most daily on the
//                schedule) every page, archived and not: marks what it sees,
//                and RECONCILES — a task with no ticket yet (a page boundary
//                the incremental walk slipped past), a linked task newer
//                than its item, or a task linked on another board (held
//                there, or taken over once that board let go) is processed
//                from the page it is on.
//   missing      linked tasks the sweep did not see get one GET /task each:
//                deleted, moved or hidden. A sweep that would mark more than a
//                fifth (and more than ten) of them marks nothing and says so.
//   queue        "Re-sync from ClickUp" on one ticket: that task, forced.
//   attachments  pending files, one at a time, each its own budget check.
//   parents      sub-tasks whose parent had no ticket when they were made.
//   remap        after a mapping edit: tickets the old mapping placed move to
//                where the new one says, unless a person has moved them since.
//   finish       watermark = the run's start (only a finished run moves it),
//                last synced, the run-level problems, one board event.
//
// BUDGETS, not batch sizes. A step stops at STEP_BUDGET_MS of elapsed time,
// when ClickUp's remaining budget falls under RATE_FLOOR, or when the driver
// signals — and ALWAYS returns Next with the advanced checkpoint. Backing off
// for the rate limit is a Next carrying `waitUntilMs`; the step after it
// answers Retry until then. Retry saves no checkpoint (talaria-runs-run's
// defer), so a Retry only ever re-enters a position already saved.
//
// AT-LEAST-ONCE. Every write is idempotent: a ticket is keyed by
// tasks.external_key (a re-entered create finds it), a comment by its
// import_key, an attachment by the item's record, and `applied` only ever
// records writes that landed — the status after its move succeeds.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use talaria_clickup::{COMMENT_PAGE, ClickUpError, Comment, Task, TaskQuery};
use talaria_runs_define::{StepError, StepResult, StepSignal};

use crate::mapping::{
    Columns, MoveVerdict, People, Placement, Problem, author_label, clamp_description, clamp_tags,
    clamp_title, guard_move, iso, map_assignees, parent_of, place, priority_of,
};
use crate::merge::{Applied, Local as LocalTicket, Upstream, merge};
use crate::store::{AttachmentState, CommentState, Finish, Item, SyncRow, Walk};
use crate::world::{Created, Local, NewTicket, Remote, Ticket, TicketPatch};
use crate::{comment_key, external_key};

/// Stop a step after this much wall time and checkpoint. A quarter of the
/// definition's max_step_ms, so one slow attachment (60 s cap) still lands
/// well inside the step deadline.
pub const STEP_BUDGET_MS: i64 = 60_000;
/// Spend ClickUp's per-minute budget down to this many requests, then wait for
/// the reset. One task can take a few requests (its detail, comment pages,
/// replies), so the floor leaves room for the one in hand.
pub const RATE_FLOOR: i64 = 8;
/// The incremental window reaches back this far past the watermark: a task
/// saved in ClickUp the moment a run started is not lost to clock skew.
pub const WATERMARK_SLACK_MS: i64 = 120_000;
/// A scheduled sync sweeps at most this often.
pub const SWEEP_EVERY_MS: i64 = 24 * 60 * 60 * 1000;
/// At most this many comments are read per task per run (40 pages).
pub const COMMENT_CAP: usize = 1000;
/// Item walks read this many rows at a time.
pub const WALK_LIMIT: i64 = 25;
/// The sweep guard: no marks when more than 1/SHARE of linked tasks, and more
/// than MIN, went missing in one sweep.
pub const SWEEP_GUARD_SHARE: i64 = 5;
pub const SWEEP_GUARD_MIN: i64 = 10;
/// A failed attachment fetch is tried on this many runs before it is listed.
pub const ATTACH_TRIES: i32 = 3;
/// A wait with no reset header.
const DEFAULT_WAIT_MS: i64 = 60_000;
const RUN_PROBLEMS_MAX: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Scheduled,
    Manual,
    /// Re-sync one ticket from ClickUp, forced.
    Ticket,
    /// Apply a mapping edit; no ClickUp calls.
    Remap,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Input {
    pub sync_id: String,
    pub mode: Mode,
    /// The ClickUp task id, for `Ticket`.
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Pass {
    #[default]
    Probe,
    Incremental,
    Archived,
    Sweep,
    Missing,
    Queue,
    Attachments,
    Parents,
    Remap,
    Finish,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Counts {
    pub checked: i64,
    pub created: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub comments: i64,
    pub attachments: i64,
    pub marked: i64,
    pub held: i64,
    pub other_board: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checkpoint {
    pub pass: Pass,
    pub page: u32,
    pub index: usize,
    /// The sweep's second half (archived tasks).
    pub archived: bool,
    pub started_at_ms: i64,
    /// This run sweeps; `swept` once both halves completed.
    pub sweep: bool,
    pub swept: bool,
    /// The missing pass checked its guard.
    pub guarded: bool,
    pub queue: Vec<String>,
    /// Keyset cursor (an external id) for the item walks.
    pub cursor: Option<String>,
    /// Back off until then (epoch ms): the rate limit.
    pub wait_until_ms: Option<i64>,
    pub counts: Counts,
    pub problems: Vec<Problem>,
    /// The mapping version the remap pass applied.
    pub remap_version: Option<i32>,
}

/// Why a unit of work stopped short.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Halt {
    /// The rate limit: come back at this epoch ms.
    Wait(i64),
    /// A failure that ends the run (a database error).
    Fatal(String),
}

/// What a pass says to the loop.
enum Flow {
    Continue,
    Yield(StepResult),
}

/// A task linked on another board, as `release` finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    /// That board still holds it.
    Held,
    /// That board let go and its ticket is gone: import it here afresh.
    Fresh,
    /// That board let go and a person moved its ticket here: adopted.
    Adopted,
}

fn hash_text(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// The first checkpoint of a run.
pub fn fresh(input: &Input, sync: &SyncRow, now: i64) -> Checkpoint {
    let first_sync = sync.watermark_ms.is_none();
    let sweep = match input.mode {
        // The first sync's incremental walk IS a full walk; its next run
        // sweeps (last_sweep_at is still null), so a page boundary it slipped
        // past heals within one interval.
        Mode::Manual => !first_sync,
        Mode::Scheduled => {
            !first_sync && sync.last_sweep_ms.is_none_or(|t| now - t > SWEEP_EVERY_MS)
        }
        Mode::Ticket | Mode::Remap => false,
    };
    Checkpoint {
        pass: match input.mode {
            Mode::Remap => Pass::Remap,
            _ => Pass::Probe,
        },
        started_at_ms: now,
        sweep,
        queue: input.external_id.clone().into_iter().collect(),
        ..Default::default()
    }
}

/// The problems an item's attachment records carry.
fn attachment_problems(item: &Item) -> Vec<Problem> {
    item.attachments
        .values()
        .filter_map(|a| match a.state.as_str() {
            "too-large" => Some(Problem::new(
                "attachment-too-large",
                format!(
                    "{} ({}) is over the 25 MB upload limit — it stays in ClickUp",
                    a.title,
                    a.size
                        .map(|s| format!("{} MB", (s + 524_288) / 1_048_576))
                        .unwrap_or_else(|| "size unknown".into())
                ),
                "Open it from the ClickUp link",
            )),
            "failed" => Some(Problem::new(
                "attachment-failed",
                format!("{} could not be copied from ClickUp", a.title),
                "Re-sync the ticket from ClickUp to try again",
            )),
            _ => None,
        })
        .collect()
}

struct Step<'a, R: Remote, L: Local> {
    run_id: &'a str,
    input: Input,
    sync: SyncRow,
    cols: Columns,
    people: People,
    cp: Checkpoint,
    local: &'a L,
    remote: Option<R>,
    began_ms: i64,
    signal: &'a StepSignal,
}

/// Drive one step. `make_remote` opens the connection's credential into a
/// ClickUp client (the fake world hands back its stand-in); a remap needs none.
pub async fn run_step<R, L, F>(
    run_id: &str,
    input: &Value,
    checkpoint: &Value,
    signal: &StepSignal,
    local: &L,
    make_remote: F,
) -> Result<StepResult, StepError>
where
    R: Remote,
    L: Local,
    F: Fn(&SyncRow) -> Result<R, String> + Send + Sync,
{
    let input: Input = serde_json::from_value(input.clone())
        .map_err(|_| "the sync run's input does not parse".to_string())?;
    let now = local.now_ms();
    let Some(sync) = local.sync(&input.sync_id).await? else {
        return Ok(StepResult::Done {
            result: json!({"stopped": "the connection no longer exists"}),
        });
    };
    if sync.state == "disconnected" {
        return Ok(StepResult::Done {
            result: json!({"stopped": "the board was disconnected from ClickUp"}),
        });
    }
    if sync.state == "paused" && input.mode == Mode::Scheduled {
        return Ok(StepResult::Done {
            result: json!({"stopped": "the sync is paused"}),
        });
    }
    let mut cp: Checkpoint = if checkpoint.is_null() {
        fresh(&input, &sync, now)
    } else {
        serde_json::from_value(checkpoint.clone())
            .map_err(|_| "the sync run's checkpoint does not parse".to_string())?
    };
    if let Some(until) = cp.wait_until_ms {
        if now < until {
            let secs = ((until - now) + 999) / 1000;
            return Ok(StepResult::Retry {
                after: Duration::from_millis((until - now) as u64),
                reason: format!("Waiting for ClickUp's rate limit — resumes in {secs}s"),
            });
        }
        cp.wait_until_ms = None;
    }
    let cols = local.columns(&sync.board_id).await?;
    let people = local.people(&sync.board_id).await?;
    let remote = if input.mode == Mode::Remap {
        None
    } else {
        match make_remote(&sync) {
            Ok(r) => Some(r),
            Err(_) => {
                let problem = Problem::new(
                    "credential",
                    "The sync has no readable ClickUp credential",
                    "A board owner replaces it in Board settings → Sync",
                );
                local.credential_rejected(&sync, &problem).await?;
                return Err(
                    "the ClickUp sync has no readable credential — a board owner replaces it"
                        .into(),
                );
            }
        }
    };
    let mut step = Step {
        run_id,
        input,
        sync,
        cols,
        people,
        cp,
        local,
        remote,
        began_ms: now,
        signal,
    };
    step.run().await
}

impl<R: Remote, L: Local> Step<'_, R, L> {
    fn remote(&self) -> Result<&R, StepError> {
        self.remote
            .as_ref()
            .ok_or_else(|| "this pass needs ClickUp and the run has no client".to_string())
    }

    fn phase(&self) -> String {
        let c = &self.cp.counts;
        match self.cp.pass {
            Pass::Probe => "Checking the ClickUp credential".into(),
            Pass::Incremental => format!(
                "Checking updated tasks — page {} · {} checked, {} new, {} updated",
                self.cp.page + 1,
                c.checked,
                c.created,
                c.updated
            ),
            Pass::Archived => format!("Checking archived tasks — page {}", self.cp.page + 1),
            Pass::Sweep => format!(
                "Reconciling every task — page {} · {} checked",
                self.cp.page + 1,
                c.checked
            ),
            Pass::Missing => format!("Checking tasks no longer in the list · {} marked", c.marked),
            Pass::Queue => "Re-syncing the ticket from ClickUp".into(),
            Pass::Attachments => format!("Copying attachments · {} copied", c.attachments),
            Pass::Parents => "Linking sub-tasks".into(),
            Pass::Remap => "Applying the new status mapping".into(),
            Pass::Finish => "Finishing".into(),
        }
    }

    fn next(&self) -> Flow {
        Flow::Yield(StepResult::Next {
            checkpoint: serde_json::to_value(&self.cp).unwrap_or(Value::Null),
            phase: Some(self.phase()),
        })
    }

    /// Back off: save the position and the time to come back.
    fn wait(&mut self, until: i64) -> Flow {
        let now = self.local.now_ms();
        self.cp.wait_until_ms = Some(until.max(now + 1_000));
        let secs = ((self.cp.wait_until_ms.unwrap_or(now) - now) + 999) / 1000;
        Flow::Yield(StepResult::Next {
            checkpoint: serde_json::to_value(&self.cp).unwrap_or(Value::Null),
            phase: Some(format!(
                "Waiting for ClickUp's rate limit — resumes in {secs}s"
            )),
        })
    }

    /// Before each unit of work: the time budget, the driver's signal, and
    /// ClickUp's remaining requests.
    fn budget(&mut self) -> Option<Flow> {
        let now = self.local.now_ms();
        if self.signal.is_aborted() || now - self.began_ms >= STEP_BUDGET_MS {
            return Some(self.next());
        }
        if let Some(r) = self.remote.as_ref().map(|r| r.rate())
            && let Some(rem) = r.remaining
            && rem < RATE_FLOOR
        {
            let until = r.reset_ms.unwrap_or(now + DEFAULT_WAIT_MS);
            if until > now {
                return Some(self.wait(until));
            }
        }
        None
    }

    fn halt(&mut self, h: Halt) -> Result<Flow, StepError> {
        match h {
            Halt::Wait(until) => Ok(self.wait(until)),
            Halt::Fatal(e) => Err(e),
        }
    }

    fn wait_of(&self, e: &ClickUpError) -> Option<i64> {
        match e {
            ClickUpError::RateLimited { reset_ms } => {
                Some(reset_ms.unwrap_or(self.local.now_ms() + DEFAULT_WAIT_MS))
            }
            _ => None,
        }
    }

    fn run_problem(&mut self, p: Problem) {
        if self.cp.problems.len() < RUN_PROBLEMS_MAX
            && !self
                .cp
                .problems
                .iter()
                .any(|q| q.kind == p.kind && q.detail == p.detail)
        {
            self.cp.problems.push(p);
        }
    }

    fn after_probe(&self) -> Pass {
        match self.input.mode {
            Mode::Ticket => Pass::Queue,
            _ => Pass::Incremental,
        }
    }

    fn after_attachments(&self) -> Pass {
        Pass::Parents
    }

    fn after_parents(&self) -> Pass {
        if self.sync.remap_pending {
            Pass::Remap
        } else {
            Pass::Finish
        }
    }

    fn enter(&mut self, pass: Pass) {
        self.cp.pass = pass;
        self.cp.page = 0;
        self.cp.index = 0;
        self.cp.cursor = None;
        self.cp.archived = false;
    }

    async fn run(&mut self) -> Result<StepResult, StepError> {
        loop {
            let flow = match self.cp.pass {
                Pass::Probe => self.probe().await?,
                Pass::Incremental => self.walk_page(Pass::Incremental).await?,
                Pass::Archived => self.walk_page(Pass::Archived).await?,
                Pass::Sweep => self.walk_page(Pass::Sweep).await?,
                Pass::Missing => self.missing().await?,
                Pass::Queue => self.queue().await?,
                Pass::Attachments => self.attachments().await?,
                Pass::Parents => self.parents().await?,
                Pass::Remap => self.remap().await?,
                Pass::Finish => return self.finish().await,
            };
            if let Flow::Yield(r) = flow {
                return Ok(r);
            }
        }
    }

    // ── probe ────────────────────────────────────────────────────────────────

    async fn probe(&mut self) -> Result<Flow, StepError> {
        let probed = self.remote()?.probe().await;
        match probed {
            Ok(_) => {}
            // THE ONE 401 THAT MEANS THE TOKEN IS DEAD. Everywhere else a 401
            // is one task out of the token's reach.
            Err(ClickUpError::Unauthorized { .. }) | Err(ClickUpError::Forbidden) => {
                let problem = Problem::new(
                    "credential",
                    "ClickUp refused the sync's credential — the token was revoked, or its ClickUp member was deactivated",
                    "A board owner replaces the credential in Board settings → Sync; the mapping is kept",
                );
                self.local.credential_rejected(&self.sync, &problem).await?;
                return Err(
                    "ClickUp refused the sync's credential — a board owner replaces it".into(),
                );
            }
            Err(e) => {
                if let Some(until) = self.wait_of(&e) {
                    return Ok(self.wait(until));
                }
                return Err(format!("the ClickUp credential could not be checked: {e}"));
            }
        }
        let list = self.remote()?.list(&self.sync.list_id).await;
        match list {
            Ok(info) => self.local.record_list(&self.sync, &info).await?,
            Err(ClickUpError::NotFound)
            | Err(ClickUpError::Forbidden)
            | Err(ClickUpError::Unauthorized { .. }) => {
                let problem = Problem::new(
                    "list-gone",
                    "The ClickUp list is gone, or the credential's member can no longer see it",
                    "Give that member access in ClickUp, or reconnect the board to another list",
                );
                self.local.run_stopped(&self.sync, &problem).await?;
                return Err(
                    "the ClickUp list is gone or not visible to the sync's credential".into(),
                );
            }
            Err(e) => {
                if let Some(until) = self.wait_of(&e) {
                    return Ok(self.wait(until));
                }
                return Err(format!("the ClickUp list could not be read: {e}"));
            }
        }
        let next = self.after_probe();
        self.enter(next);
        Ok(Flow::Continue)
    }

    // ── the page walks ───────────────────────────────────────────────────────

    async fn walk_page(&mut self, kind: Pass) -> Result<Flow, StepError> {
        let q = TaskQuery {
            page: self.cp.page,
            archived: kind == Pass::Archived || (kind == Pass::Sweep && self.cp.archived),
            updated_after_ms: if kind == Pass::Sweep {
                None
            } else {
                self.sync.watermark_ms.map(|w| w - WATERMARK_SLACK_MS)
            },
        };
        if let Some(f) = self.budget() {
            return Ok(f);
        }
        let page = match self.remote()?.tasks_page(&self.sync.list_id, &q).await {
            Ok(p) => p,
            Err(e) => {
                if let Some(until) = self.wait_of(&e) {
                    return Ok(self.wait(until));
                }
                return Err(format!("the list's tasks could not be read: {e}"));
            }
        };
        // The sweep marks what it saw and learns what it knows, in one write.
        let known: std::collections::HashMap<String, (Option<i64>, bool, String)> =
            if kind == Pass::Sweep {
                let ids: Vec<String> = page.tasks.iter().map(|t| t.id.clone()).collect();
                self.local
                    .mark_seen(&self.sync.board_id, self.run_id, &ids)
                    .await?
                    .into_iter()
                    .map(|(id, updated, tomb, board)| (id, (updated, tomb, board)))
                    .collect()
            } else {
                Default::default()
            };
        let mut wrote = false;
        while self.cp.index < page.tasks.len() {
            if let Some(f) = self.budget() {
                if wrote {
                    self.local.publish(&self.sync.board_id);
                }
                return Ok(f);
            }
            let t = &page.tasks[self.cp.index];
            let outcome = match kind {
                Pass::Archived => self.mark_archived(t).await,
                Pass::Sweep => {
                    let want = match known.get(&t.id) {
                        None => !t.archived && (!t.closed() || self.sync.options.include_closed),
                        // Linked on another board: process() holds it there
                        // (and counts it), or takes it over once that board
                        // let go.
                        Some((_, _, board)) if *board != self.sync.board_id => true,
                        Some((updated, tomb, _)) => {
                            !*tomb && (t.archived || t.updated_ms > *updated)
                        }
                    };
                    if !want {
                        Ok(false)
                    } else if t.archived {
                        self.mark_archived(t).await
                    } else {
                        self.process(t, false, false).await
                    }
                }
                _ => self.process(t, false, false).await,
            };
            match outcome {
                Ok(w) => wrote |= w,
                Err(h) => {
                    if wrote {
                        self.local.publish(&self.sync.board_id);
                    }
                    return self.halt(h);
                }
            }
            if kind != Pass::Archived {
                self.cp.counts.checked += 1;
            }
            self.cp.index += 1;
        }
        if wrote {
            self.local.publish(&self.sync.board_id);
        }
        if page.last_page {
            match kind {
                Pass::Incremental => self.enter(Pass::Archived),
                Pass::Archived => {
                    let next = if self.cp.sweep {
                        Pass::Sweep
                    } else {
                        Pass::Attachments
                    };
                    self.enter(next);
                }
                _ => {
                    if self.cp.archived {
                        self.cp.swept = true;
                        self.enter(Pass::Missing);
                    } else {
                        self.cp.page = 0;
                        self.cp.index = 0;
                        self.cp.archived = true;
                    }
                }
            }
        } else {
            self.cp.page += 1;
            self.cp.index = 0;
        }
        // A checkpoint per page: a reclaim re-reads at most this page.
        Ok(self.next())
    }

    async fn mark_archived(&mut self, t: &Task) -> Result<bool, Halt> {
        // The task's own flag, not the query that found it: archived=true is
        // ClickUp's filter, and a task it answers that says otherwise is not
        // marked.
        if !t.archived {
            return Ok(false);
        }
        let Some(mut item) = self.local.item(&t.id).await.map_err(Halt::Fatal)? else {
            return Ok(false);
        };
        if item.board_id != self.sync.board_id
            || item.tombstone()
            || item.remote_state == "archived"
        {
            return Ok(false);
        }
        item.remote_state = "archived".into();
        item.remote_updated_ms = t.updated_ms;
        self.local.save_item(&item).await.map_err(Halt::Fatal)?;
        self.cp.counts.marked += 1;
        Ok(true)
    }

    // ── one task ─────────────────────────────────────────────────────────────

    /// The ticket of a ClickUp parent, when it is on this board.
    async fn parent_ticket(&self, parent_ext: &str) -> Result<Option<String>, Halt> {
        let Some(p) = self.local.item(parent_ext).await.map_err(Halt::Fatal)? else {
            return Ok(None);
        };
        if p.board_id != self.sync.board_id {
            return Ok(None);
        }
        Ok(p.task_id)
    }

    /// A task already linked on ANOTHER board. It stays there while that
    /// board's connection still holds it, or while its ticket still sits on
    /// another board. Once the connection let go (it was disconnected) the
    /// link is free: a task whose ticket was deleted there is imported here
    /// afresh, and a ticket a person moved to this board is adopted where it
    /// sits — so "disconnect the first board, then delete its tickets or move
    /// them here" really hands a list to another board.
    async fn release(&self, it: &mut Item) -> Result<Release, Halt> {
        if let Some(owner) = &it.sync_id
            && self
                .local
                .connection_live(owner)
                .await
                .map_err(Halt::Fatal)?
        {
            return Ok(Release::Held);
        }
        let Some(task_id) = it.task_id.clone() else {
            return Ok(Release::Fresh);
        };
        match self.local.ticket(&task_id).await.map_err(Halt::Fatal)? {
            Some(tk) if tk.board_id == self.sync.board_id => {
                it.board_id = self.sync.board_id.clone();
                it.sync_id = Some(self.sync.id.clone());
                it.problems.clear();
                // Saved now: an unchanged task returns before its item is
                // written again.
                self.local.save_item(it).await.map_err(Halt::Fatal)?;
                Ok(Release::Adopted)
            }
            // Still a ticket on another board (or one this read cannot see):
            // the unique external_key would refuse a second ticket anyway.
            _ => Ok(Release::Held),
        }
    }

    /// Upsert one task's ticket. Ok(true) when anything on the board changed.
    /// `detailed`: `t` came from GET /task (so its attachments are complete),
    /// not from a list page.
    async fn process(&mut self, t: &Task, force: bool, detailed: bool) -> Result<bool, Halt> {
        let board = self.sync.board_id.clone();
        let mut existing = self.local.item(&t.id).await.map_err(Halt::Fatal)?;
        let released = match existing.as_mut() {
            Some(it) if it.board_id != board => Some(self.release(it).await?),
            _ => None,
        };
        match released {
            Some(Release::Held) => {
                // Counted once per run: by the sweep when this run sweeps
                // (it sees every task), else by the pass that met it.
                if self.cp.pass == Pass::Sweep || !self.cp.sweep {
                    self.cp.counts.other_board += 1;
                }
                return Ok(false);
            }
            Some(Release::Fresh) => existing = None,
            Some(Release::Adopted) | None => {}
        }
        if let Some(it) = &existing
            && it.tombstone()
        {
            // Deleted in Talaria: never made again.
            self.cp.counts.unchanged += 1;
            return Ok(false);
        }

        let mut problems: Vec<Problem> = Vec::new();
        let (description, cut) = clamp_description(t.description.as_deref(), t.url.as_deref());
        if cut {
            problems.push(Problem::new(
                "description-truncated",
                "The description is longer than 20,000 characters, so it was cut",
                "The full text stays in ClickUp",
            ));
        }
        let (tags, tags_cut) = clamp_tags(&t.tags);
        if tags_cut {
            problems.push(Problem::new(
                "tags-truncated",
                "Only the first 20 tags, at 40 characters each, were carried over",
                "The rest stay in ClickUp",
            ));
        }
        let (humans, mut people_problems) = map_assignees(t, &self.people);
        problems.append(&mut people_problems);
        let up = Upstream {
            title: clamp_title(&t.name),
            description,
            status: t.status.trim().to_lowercase(),
            priority: priority_of(t.priority).to_string(),
            due: iso(t.due_ms),
            start: iso(t.start_ms),
            tags,
            humans,
        };
        let placement = place(&up.status, &self.sync.status_map, &self.cols);
        match &placement {
            Placement::Unmapped(k) => problems.push(Problem::new(
                "unmapped-status",
                format!(
                    "ClickUp status \"{}\" has no column, so it sits in {}",
                    t.status,
                    self.cols.label(k)
                ),
                "Map it in Board settings → Sync",
            )),
            Placement::Nowhere => problems.push(Problem::new(
                "no-intake",
                format!(
                    "ClickUp status \"{}\" has no column and this board has no intake column without agent pickup",
                    t.status
                ),
                "Map the status, or add an open column without agent pickup",
            )),
            Placement::Mapped(_) => {}
        }

        let parent_ext = parent_of(t);
        let mut item = match existing {
            Some(it) => it,
            None => {
                if t.archived || (t.closed() && !self.sync.options.include_closed) {
                    return Ok(false); // not imported
                }
                let Some(col) = placement.column().map(str::to_string) else {
                    self.run_problem(problems.last().cloned().unwrap_or_else(|| {
                        Problem::new("no-intake", "A task has nowhere to go", "Map its status")
                    }));
                    return Ok(false);
                };
                // A ticket cannot be BORN closed: a closed task is made in the
                // intake and moved, two writes, below.
                let born = if self.cols.meta.terminal(&col) {
                    match self.cols.intake() {
                        Some(i) => i,
                        None => {
                            self.run_problem(Problem::new(
                                "no-intake",
                                "Closed ClickUp tasks need an intake column to be made in, and this board has none without agent pickup",
                                "Add an open column without agent pickup",
                            ));
                            return Ok(false);
                        }
                    }
                } else {
                    col.clone()
                };
                let parent_task = match &parent_ext {
                    Some(p) => self.parent_ticket(p).await?,
                    None => None,
                };
                let nt = NewTicket {
                    title: up.title.clone(),
                    description: up.description.clone(),
                    priority: up.priority.clone(),
                    due: up.due.clone(),
                    start: up.start.clone(),
                    tags: up.tags.clone(),
                    assignees: up.humans.clone(),
                    parent_id: parent_task.clone(),
                    status: born.clone(),
                    external_key: external_key(&t.id),
                };
                let created = self.local.create(&board, &nt).await.map_err(Halt::Fatal)?;
                let mut fresh_item = Item {
                    external_id: t.id.clone(),
                    sync_id: Some(self.sync.id.clone()),
                    board_id: board.clone(),
                    custom_id: t.custom_id.clone(),
                    external_url: t.url.clone(),
                    remote_state: "active".into(),
                    ..Default::default()
                };
                match created {
                    Created::Made(id) => {
                        fresh_item.task_id = Some(id.clone());
                        fresh_item.applied = Applied {
                            seeded: true,
                            title: up.title.clone(),
                            description: up.description.clone(),
                            priority: up.priority.clone(),
                            due: up.due.clone(),
                            start: up.start.clone(),
                            tags: up.tags.clone(),
                            humans: up.humans.clone(),
                            status: (born == col).then(|| up.status.clone()),
                            column: (born == col).then(|| col.clone()),
                            parent: parent_task.as_ref().and(parent_ext.clone()),
                            parent_pending: if parent_task.is_none() {
                                parent_ext.clone()
                            } else {
                                None
                            },
                            creator: t.creator.as_ref().map(|c| c.display()),
                            created_ms: t.created_ms,
                        };
                        // The link first: a crash from here on finds the
                        // ticket by its item (and by external_key regardless).
                        self.local
                            .save_item(&fresh_item)
                            .await
                            .map_err(Halt::Fatal)?;
                        self.cp.counts.created += 1;
                        if self.sync.options.notify_apps {
                            self.local.emit_created(&board, &id);
                        }
                        if born != col {
                            let ticket = Ticket {
                                id: id.clone(),
                                board_id: board.clone(),
                                status: born.clone(),
                                ..Default::default()
                            };
                            self.apply_status(
                                &mut fresh_item,
                                &ticket,
                                &up.status,
                                &placement,
                                &mut problems,
                            )
                            .await?;
                        }
                        self.finish_task(t, fresh_item, problems, true, force, detailed)
                            .await?;
                        return Ok(true);
                    }
                    Created::Exists(id) => {
                        // Recovered: the ticket exists, its item did not.
                        // An unseeded `applied` makes the merge a first sync.
                        fresh_item.task_id = Some(id);
                        fresh_item
                    }
                    Created::Refused(why) => {
                        self.run_problem(Problem::new(
                            "refused",
                            format!("A ClickUp task could not become a ticket: {why}"),
                            "Check the board's columns and members, then sync again",
                        ));
                        return Ok(false);
                    }
                }
            }
        };

        let task_id = item.task_id.clone().unwrap_or_default();
        let Some(ticket) = self.local.ticket(&task_id).await.map_err(Halt::Fatal)? else {
            return Ok(false);
        };
        if ticket.board_id != board {
            item.problems = vec![Problem::new(
                "moved-off-board",
                "The ticket was moved to another board, so the sync leaves it alone",
                "Move it back to sync it again",
            )];
            self.local.save_item(&item).await.map_err(Halt::Fatal)?;
            return Ok(false);
        }
        let changed = force
            || !item.applied.seeded
            || item.remote_updated_ms != t.updated_ms
            || item.remote_state != if t.archived { "archived" } else { "active" };
        if !changed {
            self.cp.counts.unchanged += 1;
            return Ok(false);
        }

        let local_view = LocalTicket {
            title: ticket.title.clone(),
            description: ticket.description.clone(),
            priority: ticket.priority.clone(),
            due: ticket.due.clone(),
            start: ticket.start.clone(),
            tags: ticket.tags.clone(),
            assignees: ticket.assignees.clone(),
        };
        let plan = merge(&up, &local_view, &item.applied, force);
        let mut wrote = false;
        if plan.writes() {
            let patch = TicketPatch {
                title: plan.title.clone(),
                description: plan.description.clone(),
                priority: plan.priority.clone(),
                due: plan.due.clone(),
                start: plan.start.clone(),
                tags: plan.tags.clone(),
                assignees: plan.assignees.clone(),
                ..Default::default()
            };
            match self
                .local
                .update(&task_id, &patch)
                .await
                .map_err(Halt::Fatal)?
            {
                Ok(()) => {
                    let (status, column) =
                        (item.applied.status.clone(), item.applied.column.clone());
                    item.applied = plan.applied.clone();
                    item.applied.status = status;
                    item.applied.column = column;
                    wrote = true;
                }
                Err(why) => problems.push(Problem::new(
                    "update-refused",
                    format!("ClickUp's changes could not be written: {why}"),
                    "Check the board's members and labels, then re-sync the ticket",
                )),
            }
        } else {
            let (status, column) = (item.applied.status.clone(), item.applied.column.clone());
            item.applied = plan.applied.clone();
            item.applied.status = status;
            item.applied.column = column;
        }
        if plan.status.is_some() {
            wrote |= self
                .apply_status(&mut item, &ticket, &up.status, &placement, &mut problems)
                .await?;
        }
        // The parent: written only when its ticket exists; pending otherwise.
        if parent_ext != item.applied.parent || force {
            match &parent_ext {
                None if ticket.parent_id.is_some() && item.applied.parent.is_some() => {
                    let patch = TicketPatch {
                        parent_id: Some(None),
                        ..Default::default()
                    };
                    if let Ok(()) = self
                        .local
                        .update(&task_id, &patch)
                        .await
                        .map_err(Halt::Fatal)?
                    {
                        item.applied.parent = None;
                        wrote = true;
                    }
                    item.applied.parent_pending = None;
                }
                None => {
                    item.applied.parent = None;
                    item.applied.parent_pending = None;
                }
                Some(p) => match self.parent_ticket(p).await? {
                    Some(pid) if ticket.parent_id.as_deref() == Some(pid.as_str()) => {
                        item.applied.parent = Some(p.clone());
                        item.applied.parent_pending = None;
                    }
                    Some(pid) => {
                        let patch = TicketPatch {
                            parent_id: Some(Some(pid)),
                            ..Default::default()
                        };
                        match self.local.update(&task_id, &patch).await.map_err(Halt::Fatal)? {
                            Ok(()) => {
                                item.applied.parent = Some(p.clone());
                                wrote = true;
                            }
                            Err(why) => problems.push(Problem::new(
                                "parent-refused",
                                format!("It is a sub-task in ClickUp but could not be nested here: {why}"),
                                "Nest it by hand, or leave it top-level",
                            )),
                        }
                        item.applied.parent_pending = None;
                    }
                    None => item.applied.parent_pending = Some(p.clone()),
                },
            }
        }
        self.finish_task(t, item, problems, false, force, detailed)
            .await?;
        if wrote {
            self.cp.counts.updated += 1;
        } else {
            self.cp.counts.unchanged += 1;
        }
        Ok(true)
    }

    /// Place the ticket where ClickUp's status says, through the guards.
    /// `applied.status` and `applied.column` are written only when the move
    /// landed (or the ticket was already there).
    async fn apply_status(
        &mut self,
        item: &mut Item,
        ticket: &Ticket,
        status: &str,
        placement: &Placement,
        problems: &mut Vec<Problem>,
    ) -> Result<bool, Halt> {
        let Some(col) = placement.column() else {
            return Ok(false);
        };
        let has_agents = ticket
            .assignees
            .iter()
            .any(|a| !talaria_tasks_types::is_human_assignee(a));
        match guard_move(
            &ticket.status,
            col,
            &self.cols,
            has_agents,
            ticket.live_session,
        ) {
            MoveVerdict::Stay => {
                item.applied.status = Some(status.to_string());
                item.applied.column = Some(col.to_string());
                Ok(false)
            }
            MoveVerdict::Hold(why) => {
                self.cp.counts.held += 1;
                problems.push(Problem::new(
                    "status-held",
                    why,
                    "Move it yourself when it is ready — the next change in ClickUp asks again",
                ));
                Ok(false)
            }
            MoveVerdict::Move => {
                let note = if self.cols.meta.terminal(col) {
                    "closed in ClickUp".to_string()
                } else {
                    format!("ClickUp status \"{status}\"")
                };
                let patch = TicketPatch {
                    status: Some(col.to_string()),
                    status_note: Some(note),
                    ..Default::default()
                };
                match self
                    .local
                    .update(&ticket.id, &patch)
                    .await
                    .map_err(Halt::Fatal)?
                {
                    Ok(()) => {
                        item.applied.status = Some(status.to_string());
                        item.applied.column = Some(col.to_string());
                        // A quiet write emits no TicketMoved; the connection
                        // decides whether apps hear the sync's moves.
                        if self.sync.options.notify_apps {
                            self.local.emit_moved(
                                &self.sync.board_id,
                                &ticket.id,
                                &ticket.status,
                                col,
                                self.cols.meta.terminal(col),
                            );
                        }
                        Ok(true)
                    }
                    Err(why) => {
                        problems.push(Problem::new(
                            "status-refused",
                            format!("ClickUp's status could not be applied: {why}"),
                            "Move it yourself, or fix the mapping in Board settings → Sync",
                        ));
                        Ok(false)
                    }
                }
            }
        }
    }

    /// Comments and attachments, then the item row. A transient ClickUp
    /// failure on either keeps the item's `remote_updated_ms` where it was, so
    /// the sweep retries the task.
    async fn finish_task(
        &mut self,
        t: &Task,
        mut item: Item,
        mut problems: Vec<Problem>,
        created: bool,
        force: bool,
        detailed: bool,
    ) -> Result<(), Halt> {
        let task_id = item.task_id.clone().unwrap_or_default();
        let mut complete = true;
        if self.sync.options.comments {
            match self.import_comments(t, &task_id, &mut item).await {
                Ok(n) => {
                    if n > 0 {
                        self.cp.counts.comments += n;
                        let line = if n == 1 {
                            "imported 1 comment from ClickUp".to_string()
                        } else {
                            format!("imported {n} comments from ClickUp")
                        };
                        self.local
                            .activity(&task_id, &line)
                            .await
                            .map_err(Halt::Fatal)?;
                    }
                }
                Err(CommentFail::Halt(h)) => {
                    // Save what landed before backing off: the comment map
                    // keeps a re-entry from re-reading what it already holds.
                    self.local.save_item(&item).await.map_err(Halt::Fatal)?;
                    return Err(h);
                }
                Err(CommentFail::Capped) => problems.push(Problem::new(
                    "comments-capped",
                    format!("Only the latest {COMMENT_CAP} comments were imported"),
                    "The rest stay in ClickUp",
                )),
                Err(CommentFail::Unreadable) => {
                    complete = false;
                    problems.push(Problem::new(
                        "comments-unreadable",
                        "The comments could not be read from ClickUp this time",
                        "The next sync tries again",
                    ));
                }
            }
        }
        if self.sync.options.attachments
            && (created || force || item.remote_updated_ms != t.updated_ms)
        {
            // ONLY A DETAIL READ'S FILES COUNT. A list page may carry an
            // `attachments` field that is partial or empty (it is not where
            // ClickUp lists files); trusting it would drop files silently.
            let detail = if detailed && t.attachments.is_some() {
                Ok(t.clone())
            } else {
                self.remote().map_err(Halt::Fatal)?.task(&t.id).await
            };
            match detail {
                Ok(d) => {
                    for a in d.attachments.unwrap_or_default() {
                        if item.attachments.contains_key(&a.id) {
                            continue;
                        }
                        let too_large = a
                            .size
                            .is_some_and(|s| s as usize > talaria_uploads::MAX_BYTES);
                        let state = if too_large {
                            "too-large"
                        } else if a.url.is_none() {
                            "failed"
                        } else {
                            "pending"
                        };
                        item.attachments.insert(
                            a.id.clone(),
                            AttachmentState {
                                title: a.title.clone(),
                                size: a.size,
                                mime: a.mime.clone(),
                                url: a.url.clone(),
                                state: state.into(),
                                upload_id: None,
                                tries: 0,
                            },
                        );
                    }
                }
                Err(e) => {
                    if let Some(until) = self.wait_of(&e) {
                        self.local.save_item(&item).await.map_err(Halt::Fatal)?;
                        return Err(Halt::Wait(until));
                    }
                    complete = false;
                }
            }
        }
        item.custom_id = t.custom_id.clone();
        item.external_url = t.url.clone();
        item.sync_id = Some(self.sync.id.clone());
        item.remote_state = if t.archived { "archived" } else { "active" }.into();
        if complete {
            item.remote_updated_ms = t.updated_ms;
        }
        problems.extend(attachment_problems(&item));
        item.problems = problems;
        self.local.save_item(&item).await.map_err(Halt::Fatal)
    }

    /// Gather the task's comments across pages, then land them OLDEST FIRST:
    /// a room is ordered by seq, assigned at insert, so newest-first inserts
    /// would reverse the history. A comment whose text changed in ClickUp is
    /// mirrored; replies are read where the count grew, and only then — a
    /// reply edited in ClickUp afterwards is not mirrored (re-reading every
    /// thread each run would cost a request per comment). The count landed.
    async fn import_comments(
        &self,
        t: &Task,
        task_id: &str,
        item: &mut Item,
    ) -> Result<i64, CommentFail> {
        let remote = self
            .remote()
            .map_err(|e| CommentFail::Halt(Halt::Fatal(e)))?;
        let mut all: Vec<Comment> = Vec::new();
        let mut before: Option<(i64, String)> = None;
        let mut capped = false;
        loop {
            let batch = match remote.comments(&t.id, before.clone()).await {
                Ok(b) => b,
                Err(e) => {
                    return Err(match self.wait_of(&e) {
                        Some(until) => CommentFail::Halt(Halt::Wait(until)),
                        None => CommentFail::Unreadable,
                    });
                }
            };
            let n = batch.len();
            let next = batch.last().map(|c| (c.date_ms.unwrap_or(0), c.id.clone()));
            for c in batch {
                if !all.iter().any(|a| a.id == c.id) {
                    all.push(c);
                }
            }
            if n < COMMENT_PAGE || next.is_none() || next == before {
                break;
            }
            if all.len() >= COMMENT_CAP {
                capped = true;
                break;
            }
            before = next;
        }
        all.sort_by(|a, b| (a.date_ms.unwrap_or(0), &a.id).cmp(&(b.date_ms.unwrap_or(0), &b.id)));
        let mut landed = 0;
        for c in &all {
            let key = comment_key(&c.id);
            let label = author_label(c.user.as_ref());
            let hash = hash_text(&c.text);
            let state = item.comments.get(&c.id).cloned();
            let mut st = match state {
                None => {
                    let Some((id, inserted)) = self
                        .local
                        .import_comment(task_id, &label, &c.text, c.date_ms, None, &key)
                        .await
                        .map_err(|e| CommentFail::Halt(Halt::Fatal(e)))?
                    else {
                        return Err(CommentFail::Unreadable);
                    };
                    if inserted {
                        landed += 1;
                    }
                    CommentState {
                        hash: hash.clone(),
                        replies: 0,
                        message_id: id,
                    }
                }
                Some(mut s) => {
                    if s.hash != hash {
                        self.local
                            .edit_comment(task_id, &key, &c.text)
                            .await
                            .map_err(|e| CommentFail::Halt(Halt::Fatal(e)))?;
                        s.hash = hash.clone();
                    }
                    s
                }
            };
            if c.reply_count > st.replies {
                let replies = match remote.replies(&c.id).await {
                    Ok(r) => r,
                    Err(e) => {
                        item.comments.insert(c.id.clone(), st);
                        return Err(match self.wait_of(&e) {
                            Some(until) => CommentFail::Halt(Halt::Wait(until)),
                            None => CommentFail::Unreadable,
                        });
                    }
                };
                let mut replies = replies;
                replies.sort_by(|a, b| {
                    (a.date_ms.unwrap_or(0), &a.id).cmp(&(b.date_ms.unwrap_or(0), &b.id))
                });
                for r in &replies {
                    let rkey = comment_key(&r.id);
                    let reply = self
                        .local
                        .import_comment(
                            task_id,
                            &author_label(r.user.as_ref()),
                            &r.text,
                            r.date_ms,
                            Some(&st.message_id),
                            &rkey,
                        )
                        .await
                        .map_err(|e| CommentFail::Halt(Halt::Fatal(e)))?;
                    if matches!(reply, Some((_, true))) {
                        landed += 1;
                    }
                }
                st.replies = c.reply_count;
            }
            item.comments.insert(c.id.clone(), st);
        }
        if capped {
            return Err(CommentFail::Capped);
        }
        Ok(landed)
    }

    // ── missing ──────────────────────────────────────────────────────────────

    async fn missing(&mut self) -> Result<Flow, StepError> {
        if !self.cp.swept {
            self.enter(Pass::Attachments);
            return Ok(Flow::Continue);
        }
        if !self.cp.guarded {
            let (total, unseen) = self.local.unseen_counts(&self.sync.id, self.run_id).await?;
            self.cp.guarded = true;
            if unseen > SWEEP_GUARD_MIN && unseen * SWEEP_GUARD_SHARE > total {
                self.run_problem(Problem::new(
                    "sweep-held",
                    format!(
                        "{unseen} of {total} synced tasks were not in the list this time, so none were marked — that looks like a permission or ClickUp hiccup, not deletions"
                    ),
                    "Sync now again later; if it persists, check the credential's access to the list",
                ));
                self.enter(Pass::Attachments);
                return Ok(Flow::Continue);
            }
        }
        let batch = self
            .local
            .walk(
                &self.sync.id,
                Walk::Unseen,
                self.run_id,
                self.cp.cursor.as_deref(),
                WALK_LIMIT,
            )
            .await?;
        let full = batch.len() as i64 == WALK_LIMIT;
        for mut item in batch {
            if let Some(f) = self.budget() {
                return Ok(f);
            }
            let read = self.remote()?.task(&item.external_id).await;
            let state = match read {
                Ok(t) if t.in_list(&self.sync.list_id) => {
                    if t.archived {
                        Some("archived")
                    } else {
                        None
                    }
                }
                Ok(_) => Some("moved"),
                Err(ClickUpError::NotFound) => Some("deleted"),
                Err(ClickUpError::Unauthorized { .. }) | Err(ClickUpError::Forbidden) => {
                    Some("hidden")
                }
                Err(e) => {
                    if let Some(until) = self.wait_of(&e) {
                        return Ok(self.wait(until));
                    }
                    None
                }
            };
            if let Some(s) = state {
                item.remote_state = s.into();
                self.local.save_item(&item).await?;
                self.cp.counts.marked += 1;
            }
            self.cp.cursor = Some(item.external_id.clone());
        }
        if !full {
            self.local.publish(&self.sync.board_id);
            self.enter(Pass::Attachments);
            return Ok(Flow::Continue);
        }
        Ok(self.next())
    }

    // ── queue (re-sync one ticket) ───────────────────────────────────────────

    async fn queue(&mut self) -> Result<Flow, StepError> {
        while self.cp.index < self.cp.queue.len() {
            if let Some(f) = self.budget() {
                return Ok(f);
            }
            let id = self.cp.queue[self.cp.index].clone();
            match self.remote()?.task(&id).await {
                Ok(t) => {
                    if let Err(h) = self.process(&t, true, true).await {
                        return self.halt(h);
                    }
                }
                Err(ClickUpError::NotFound) => {
                    if let Some(mut item) = self.local.item(&id).await?
                        && item.board_id == self.sync.board_id
                    {
                        item.remote_state = "deleted".into();
                        self.local.save_item(&item).await?;
                        self.cp.counts.marked += 1;
                    }
                }
                Err(e) => {
                    if let Some(until) = self.wait_of(&e) {
                        return Ok(self.wait(until));
                    }
                    return Err(format!("the task could not be read from ClickUp: {e}"));
                }
            }
            self.cp.index += 1;
        }
        self.local.publish(&self.sync.board_id);
        self.enter(Pass::Attachments);
        Ok(Flow::Continue)
    }

    // ── attachments ──────────────────────────────────────────────────────────

    async fn attachments(&mut self) -> Result<Flow, StepError> {
        if !self.sync.options.attachments {
            let next = self.after_attachments();
            self.enter(next);
            return Ok(Flow::Continue);
        }
        let batch = self
            .local
            .walk(
                &self.sync.id,
                Walk::PendingAttachments,
                self.run_id,
                self.cp.cursor.as_deref(),
                WALK_LIMIT,
            )
            .await?;
        let full = batch.len() as i64 == WALK_LIMIT;
        for mut item in batch {
            let task_id = item.task_id.clone().unwrap_or_default();
            let pending: Vec<String> = item
                .attachments
                .iter()
                .filter(|(_, a)| a.state == "pending")
                .map(|(id, _)| id.clone())
                .collect();
            for att_id in pending {
                if let Some(f) = self.budget() {
                    return Ok(f);
                }
                let Some(a) = item.attachments.get(&att_id).cloned() else {
                    continue;
                };
                let url = a.url.clone().unwrap_or_default();
                let got = self
                    .remote()?
                    .download(&url, talaria_uploads::MAX_BYTES)
                    .await;
                let mut next = a.clone();
                match got {
                    Ok(bytes) => {
                        let mime = a
                            .mime
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".into());
                        match self.local.attach(&task_id, &a.title, &mime, bytes).await {
                            Ok(upload) => {
                                next.state = "done".into();
                                next.upload_id = Some(upload);
                                self.cp.counts.attachments += 1;
                            }
                            Err(_) => {
                                next.tries += 1;
                                if next.tries >= ATTACH_TRIES {
                                    next.state = "failed".into();
                                }
                            }
                        }
                    }
                    Err(ClickUpError::TooLarge) => next.state = "too-large".into(),
                    Err(e) => {
                        if let Some(until) = self.wait_of(&e) {
                            return Ok(self.wait(until));
                        }
                        next.tries += 1;
                        if next.tries >= ATTACH_TRIES
                            || matches!(e, ClickUpError::NotFound | ClickUpError::Forbidden)
                        {
                            next.state = "failed".into();
                        }
                    }
                }
                // A file that failed this time and has tries left waits for
                // the next run; this run moves past it either way.
                item.attachments.insert(att_id.clone(), next);
                item.problems.retain(|p| !p.kind.starts_with("attachment-"));
                item.problems.extend(attachment_problems(&item));
                self.local.save_item(&item).await?;
            }
            self.cp.cursor = Some(item.external_id.clone());
        }
        if !full {
            self.local.publish(&self.sync.board_id);
            let next = self.after_attachments();
            self.enter(next);
            return Ok(Flow::Continue);
        }
        Ok(self.next())
    }

    // ── parents ──────────────────────────────────────────────────────────────

    async fn parents(&mut self) -> Result<Flow, StepError> {
        let batch = self
            .local
            .walk(
                &self.sync.id,
                Walk::PendingParents,
                self.run_id,
                self.cp.cursor.as_deref(),
                WALK_LIMIT,
            )
            .await?;
        let full = batch.len() as i64 == WALK_LIMIT;
        for mut item in batch {
            if let Some(f) = self.budget() {
                return Ok(f);
            }
            self.cp.cursor = Some(item.external_id.clone());
            let Some(parent_ext) = item.applied.parent_pending.clone() else {
                continue;
            };
            let parent = match self.parent_ticket(&parent_ext).await {
                Ok(p) => p,
                Err(Halt::Fatal(e)) => return Err(e),
                Err(Halt::Wait(u)) => return Ok(self.wait(u)),
            };
            let Some(pid) = parent else {
                continue; // its parent has no ticket on this board yet
            };
            let task_id = item.task_id.clone().unwrap_or_default();
            let patch = TicketPatch {
                parent_id: Some(Some(pid)),
                ..Default::default()
            };
            match self.local.update(&task_id, &patch).await? {
                Ok(()) => item.applied.parent = Some(parent_ext),
                Err(why) => item.problems.push(Problem::new(
                    "parent-refused",
                    format!("It is a sub-task in ClickUp but could not be nested here: {why}"),
                    "Nest it by hand, or leave it top-level",
                )),
            }
            item.applied.parent_pending = None;
            self.local.save_item(&item).await?;
        }
        if !full {
            let next = self.after_parents();
            self.enter(next);
            return Ok(Flow::Continue);
        }
        Ok(self.next())
    }

    // ── remap ────────────────────────────────────────────────────────────────

    async fn remap(&mut self) -> Result<Flow, StepError> {
        if self.cp.remap_version.is_none() {
            self.cp.remap_version = Some(self.sync.map_version);
        }
        let batch = self
            .local
            .walk(
                &self.sync.id,
                Walk::Linked,
                self.run_id,
                self.cp.cursor.as_deref(),
                WALK_LIMIT,
            )
            .await?;
        let full = batch.len() as i64 == WALK_LIMIT;
        let mut wrote = false;
        for mut item in batch {
            if let Some(f) = self.budget() {
                if wrote {
                    self.local.publish(&self.sync.board_id);
                }
                return Ok(f);
            }
            self.cp.cursor = Some(item.external_id.clone());
            let (Some(status), Some(column)) =
                (item.applied.status.clone(), item.applied.column.clone())
            else {
                continue;
            };
            let placement = place(&status, &self.sync.status_map, &self.cols);
            let Some(target) = placement.column() else {
                continue;
            };
            if target == column {
                continue;
            }
            let task_id = item.task_id.clone().unwrap_or_default();
            let Some(ticket) = self.local.ticket(&task_id).await? else {
                continue;
            };
            // A person moved it since the sync placed it: theirs stands.
            if ticket.status != column || ticket.board_id != self.sync.board_id {
                continue;
            }
            let mut problems = item
                .problems
                .iter()
                .filter(|p| p.kind != "unmapped-status" && p.kind != "status-held")
                .cloned()
                .collect::<Vec<_>>();
            if let Placement::Unmapped(k) = &placement {
                problems.push(Problem::new(
                    "unmapped-status",
                    format!(
                        "ClickUp status \"{status}\" has no column, so it sits in {}",
                        self.cols.label(k)
                    ),
                    "Map it in Board settings → Sync",
                ));
            }
            let moved = match self
                .apply_status(&mut item, &ticket, &status, &placement, &mut problems)
                .await
            {
                Ok(m) => m,
                Err(Halt::Fatal(e)) => return Err(e),
                Err(Halt::Wait(u)) => return Ok(self.wait(u)),
            };
            wrote |= moved;
            item.problems = problems;
            self.local.save_item(&item).await?;
        }
        if !full {
            if wrote {
                self.local.publish(&self.sync.board_id);
            }
            self.enter(Pass::Finish);
            return Ok(Flow::Continue);
        }
        if wrote {
            self.local.publish(&self.sync.board_id);
        }
        Ok(self.next())
    }

    // ── finish ───────────────────────────────────────────────────────────────

    async fn finish(&mut self) -> Result<StepResult, StepError> {
        if self.cp.counts.other_board > 0 {
            let n = self.cp.counts.other_board;
            self.run_problem(Problem::new(
                "other-board",
                format!(
                    "{n} task{} already {} to another board, so {} skipped",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "belongs" } else { "belong" },
                    if n == 1 { "it was" } else { "they were" }
                ),
                "A ClickUp task feeds one board: disconnect the other board's sync, then delete those tickets there or move them to this board — the next sync takes them over",
            ));
        }
        let walked = matches!(self.input.mode, Mode::Scheduled | Mode::Manual);
        let f = Finish {
            watermark_ms: walked.then_some(self.cp.started_at_ms),
            walked,
            swept: self.cp.swept,
            remap_version: self.cp.remap_version,
            problems: self.cp.problems.clone(),
        };
        self.local.finish(&self.sync, &f).await?;
        self.local.publish(&self.sync.board_id);
        let c = &self.cp.counts;
        Ok(StepResult::Done {
            result: json!({
                "mode": self.input.mode,
                "checked": c.checked,
                "created": c.created,
                "updated": c.updated,
                "unchanged": c.unchanged,
                "comments": c.comments,
                "attachments": c.attachments,
                "marked": c.marked,
                "held": c.held,
                "problems": self.cp.problems.len(),
                "swept": self.cp.swept,
            }),
        })
    }
}

/// Why the comment import stopped short.
enum CommentFail {
    Halt(Halt),
    /// The cap: everything up to it landed.
    Capped,
    /// ClickUp would not answer for this task; retried next time.
    Unreadable,
}

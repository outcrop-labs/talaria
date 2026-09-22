// Board workchains — branching ticket graphs (A → B, A → C) with human/agent
// handoffs (TALA-30 v1 linear; TALA-35 the DAG + fan-out).
//
// The model is EDGES (from_step → to_step), not an ordered list: a workchain
// is a DAG the routes keep acyclic on write. The step state is derived, never
// stored — per step, from its predecessors:
//
//   archived            the ticket was archived (the chain reads past it)
//   done                its task sits in a done-category column (the one
//                       definition, StatusMeta::terminal, off-board included —
//                       a failed step did not complete, but it is not live)
//   blocked             some predecessor is still live — the AND-join: every
//                       predecessor must finish before this step may start
//   head                no predecessor at all (a chain entry point)
//   ready               every predecessor is done/archived, and it has at
//                       least one — the fan-out made it this step's turn
//
// A linear chain (the v1 shape, still the common case) derives exactly as v1
// did: one head, later steps waiting — 'blocked' here, 'waiting' on the wire
// name kept for the v1 UI vocabulary. The canvas (TALA-35) reads the finer
// states; the rails keep rendering 'waiting' for anything not head/done.
//
// ── The handoff engine ────────────────────────────────────────────────────────
//
// advance_workchains is the write-side half of the same derived state: when a
// task write lands a step's ticket in a DONE column, EVERY successor whose
// predecessors are now all satisfied becomes ready at once (fan-out), and each
// one's human assignees are told it is their turn. Agents are not — their
// visibility is the heartbeat, which serves ready heads and hides blocked
// steps, so an agent cannot read its chain position as an invitation to start
// early. A step with several predecessors stays blocked until the LAST of them
// completes — the notification rides that final edge, and only that firing
// notifies (a successor whose other predecessors were already done does not
// re-notify per completing predecessor).
//
// An off-board terminal (failed/cancelled) pauses the whole chain and tells
// its creator — v1's semantics, kept for v1 (the subtree-vs-chain question is
// triage's to settle; nothing here stores pause scope).
//
// The engine fires on the ordinary status-write path only — the one door
// update_task is — so a human sign-off (or a reviewer's approve) is the only
// trigger. Agents cannot land a terminal column (the MCP guardrail), which
// makes human approval the sole way a chain advances: nothing else calls
// this, on purpose.
//
// IDEMPOTENT BY CONSTRUCTION: no chain state lives in memory. Each firing
// re-reads the graph from the DB and notifies the successors that are NOW
// ready AND hang off the step that just completed — a retried status write
// re-derives the same answer (and re-notifies, the same quiet double the v1
// transition ride accepted; the transition is the trigger, not stored state).

use sqlx::PgPool;
use talaria_notify::{NotificationInput, NotifyDeps};
use talaria_statuses::{OFF_BOARD_STATUSES, StatusMeta, status_meta};

use futures_util::future::BoxFuture;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use talaria_tasks_types::Task;

pub static GET_TASK: OnceLock<
    Arc<
        dyn Fn(sqlx::PgPool, String) -> BoxFuture<'static, Result<Option<Task>, sqlx::Error>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

/// A workchain on the wire: id, boardId, name, createdBy, paused, position,
/// createdAt, updatedAt, then its steps with each step's task summary, the
/// derived state, the canvas placement, and the wires (edges) as task-id
/// pairs — the canvas draws nodes from the steps and wires from the edges.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workchain {
    pub id: String,
    pub board_id: String,
    pub name: String,
    pub created_by: Option<String>,
    pub paused: bool,
    pub position: i32,
    pub created_at: String,
    pub updated_at: String,
    pub steps: Vec<WorkchainStep>,
    /// The graph: each wire names its endpoints by TASK id (the cards' key),
    /// in from → to order. Derived from task_workchain_edges per read.
    pub edges: Vec<WorkchainEdge>,
}

/// One wire: the two endpoints, as TASK ids (steps are 1:1 with tasks — the
/// unique index says so — so task ids are the stable handle every lens uses).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkchainEdge {
    pub from_task_id: String,
    pub to_task_id: String,
}

/// One step: its task's summary — the fields a chain card renders (ref,
/// title, assignees, effort, due, status, archived) — plus `state`, the
/// derived done/head/ready/blocked/waiting/archived, and the canvas x/y
/// (None = never placed; the view auto-layouts). Deliberately not the full
/// Task: a chain is a reading surface; the card link opens the ticket.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkchainStep {
    pub task_id: String,
    pub position: i32,
    /// 'done' | 'head' | 'ready' | 'blocked' | 'archived' — derived per
    /// read, never stored. ('waiting' rode the v1 wire; the api maps it to
    /// 'blocked' — see derive doc at the top of this file.)
    pub state: &'static str,
    pub ticket_ref: Option<String>,
    pub title: String,
    pub assignees: Vec<String>,
    pub effort: Option<String>,
    pub due_date: Option<String>,
    pub status: String,
    pub archived: bool,
    /// Free canvas placement, x then y. Null on both = the view lays out.
    pub x: Option<i32>,
    pub y: Option<i32>,
}

/// The chain row in select order — id, boardId, name, createdBy, paused,
/// position, then created/updated as epoch-ms (shaped to ISO on the wire the
/// same way every board surface shapes them).
type ChainRow = (String, String, String, Option<String>, bool, i32, i64, i64);

/// A step joined to its task summary, in select order. The workchain id is
/// the FIRST column: the list reads every chain's steps in one query and
/// groups by it in Rust — ordering by workchain first is what keeps that a
/// single pass.
type StepRow = (
    String,
    String,
    i32,
    Option<String>,
    String,
    serde_json::Value,
    Option<String>,
    Option<i64>,
    String,
    Option<i64>,
    Option<i32>,
    Option<i32>,
);

/// An edge row: workchain id, from-step's TASK id, to-step's TASK id — the
/// wire shape the whole read side speaks (steps are keyed by task everywhere
/// on the wire; step uuids never leave the database).
type EdgeRow = (String, String, String);

fn iso(ms: i64) -> String {
    talaria_agent_auth::epoch_ms_to_iso(ms)
}

/// A predecessor is satisfied when its work is OFF the table: a done-category
/// column (the work completed) or an archived ticket (the chain reads past
/// it — the derive below and the v1 head rule agree). A failed/cancelled
/// predecessor is neither: the work did not complete, so the join waits —
/// and the engine has already paused the chain and told its creator.
fn pred_satisfied(archived: bool, status: &str, meta: &StatusMeta) -> bool {
    archived || terminal_of(meta, status) == Terminal::Done
}

/// The one derive, split out so the state rules are testable against a
/// synthetic graph. Steps carry their predecessor list (task id → archived +
/// status); a step is:
///   - archived, when its ticket is archived (chain structure, read past),
///   - done, when its task is terminal (done column or off-board),
///   - blocked, when ANY predecessor is still live (not archived, not
///     terminal) — the AND-join waiting on its last open predecessor,
///   - head, when it has NO predecessors (an entry point), or when ONE
///     predecessor completed and handed the baton to this step ALONE —
///     the hand-off of a straight line, the one live step the v1 chain
///     already called its head, else
///   - ready — every predecessor finished otherwise: a fan-out (several
///     successors at once), an AND-join closing, an archived-skip. Only a
///     BRANCH produces these — so the v1 UI vocabulary (done/head/waiting)
///     is preserved exactly on a straight line, and 'ready' says the
///     fan-out or the join made it this step's turn.
fn derive_state(
    meta: &StatusMeta,
    archived: bool,
    status: &str,
    preds_ready: bool,
    has_preds: bool,
    baton: bool,
) -> &'static str {
    if archived {
        "archived"
    } else if meta.terminal(status) {
        "done"
    } else if !has_preds || baton {
        "head"
    } else if preds_ready {
        "ready"
    } else {
        "blocked"
    }
}

/// A step from its row: every field is carried, the state is a placeholder —
/// the chain read derives it below, from the StatusMeta and the edges.
fn step_of(row: StepRow) -> WorkchainStep {
    let (
        _,
        task_id,
        position,
        ticket_ref,
        title,
        assignees,
        effort,
        due_ms,
        status,
        archived_ms,
        x,
        y,
    ) = row;
    let archived = archived_ms.is_some();
    WorkchainStep {
        task_id,
        position,
        // Placeholder: the chain read assigns every step's state below.
        state: "blocked",
        ticket_ref,
        title,
        assignees: talaria_tasks_types::json_strings(&assignees),
        effort,
        due_date: due_ms.map(iso),
        status,
        archived,
        x,
        y,
    }
}

/// The board's workchains in position order, each with its steps joined to
/// task summaries, the derived states, and its wires. Three queries: the
/// chains, every chain's steps in one pass grouped by workchain id, and every
/// chain's edges in one pass.
pub async fn list_workchains(pg: &PgPool, board_id: &str) -> Result<Vec<Workchain>, sqlx::Error> {
    let chains: Vec<ChainRow> = sqlx::query_as(
        "select id::text, board_id::text, name, created_by, paused, position, \
                (trunc(extract(epoch from created_at) * 1000))::bigint, \
                (trunc(extract(epoch from updated_at) * 1000))::bigint \
         from task_workchains where board_id = $1::uuid order by position, created_at",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    let steps: Vec<StepRow> = sqlx::query_as(
        "select s.workchain_id::text, s.task_id::text, s.position, \
                case when t.ticket_no is not null \
                     then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end, \
                t.title, t.assignees, t.effort, \
                (trunc(extract(epoch from t.due_date) * 1000))::bigint, \
                t.status, \
                (trunc(extract(epoch from t.archived_at) * 1000))::bigint, \
                s.canvas_x, s.canvas_y \
         from task_workchain_steps s \
         join task_workchains w on w.id = s.workchain_id \
         join tasks t on t.id = s.task_id \
         join boards b on b.id = t.board_id \
         where w.board_id = $1::uuid \
         order by s.workchain_id, s.position, s.created_at",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    let edges: Vec<EdgeRow> = sqlx::query_as(
        "select e.workchain_id::text, ft.task_id::text, tt.task_id::text \
         from task_workchain_edges e \
         join task_workchain_steps ft on ft.id = e.from_step \
         join task_workchain_steps ts on ts.id = e.to_step \
         join task_workchains w on w.id = e.workchain_id \
         where w.board_id = $1::uuid \
         order by e.created_at, e.id",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    let meta = status_meta(pg, board_id).await?;
    let mut by_chain: HashMap<String, Vec<WorkchainStep>> = HashMap::new();
    for row in steps {
        let chain = row.0.clone();
        by_chain.entry(chain).or_default().push(step_of(row));
    }
    // The edges grouped per chain, as task-id pairs; a step's predecessors
    // come straight off them (the derive walks successors of the map, but
    // the state rule is a predecessor count).
    let mut edges_by_chain: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (chain, from, to) in edges {
        edges_by_chain.entry(chain).or_default().push((from, to));
    }
    Ok(chains
        .into_iter()
        .map(
            |(id, board_id, name, created_by, paused, position, created_ms, updated_ms)| {
                let mut steps = by_chain.remove(&id).unwrap_or_default();
                let chain_edges = edges_by_chain.remove(&id).unwrap_or_default();
                derive_states(&meta, &mut steps, &chain_edges);
                Workchain {
                    id,
                    board_id,
                    name,
                    created_by,
                    paused,
                    position,
                    created_at: iso(created_ms),
                    updated_at: iso(updated_ms),
                    steps,
                    edges: chain_edges
                        .into_iter()
                        .map(|(from, to)| WorkchainEdge {
                            from_task_id: from,
                            to_task_id: to,
                        })
                        .collect(),
                }
            },
        )
        .collect())
}

/// Assign every step's state from the graph: predecessors per task off the
/// edge list, then the one derive rule per step. Steps not touched by an
/// edge have none (heads of their chain, or a lone step).
fn derive_states(meta: &StatusMeta, steps: &mut [WorkchainStep], edges: &[(String, String)]) {
    // An OWNED snapshot, built before the mutable pass: nothing borrows
    // `steps` across its own mutation. Task id → (archived, status).
    let snap: HashMap<String, (bool, String)> = steps
        .iter()
        .map(|s| (s.task_id.clone(), (s.archived, s.status.clone())))
        .collect();
    // Predecessor table off the edges and that snapshot: task id → each
    // pred's (id, archived, status) — the id keys the out-degree walk.
    let mut preds: HashMap<String, Vec<(String, bool, String)>> = HashMap::new();
    for (from, to) in edges {
        let (Some(f), Some(_)) = (snap.get(from), snap.get(to)) else {
            continue; // an edge naming a task this read does not carry (filtered view) — state-irrelevant
        };
        preds
            .entry(to.clone())
            .or_default()
            .push((from.clone(), f.0, f.1.clone()));
    }
    // Out-degree per task: how many successors a step hands off to. A baton
    // hand-off is out-degree 1 — the completing pred handed the chain to
    // this step ALONE (fan-out is out-degree ≥ 2, and ready).
    let out_degree: HashMap<&str, usize> = edges.iter().fold(HashMap::new(), |mut m, (from, _)| {
        *m.entry(from.as_str()).or_insert(0) += 1;
        m
    });
    for step in steps.iter_mut() {
        let plist = preds.get(&step.task_id);
        let satisfied = plist
            .map(|ps| {
                ps.iter()
                    .all(|(_, archived, status)| pred_satisfied(*archived, status, meta))
            })
            .unwrap_or(true);
        let has_preds = plist.map(|p| !p.is_empty()).unwrap_or(false);
        // The baton: exactly one pred, it completed (archived is a skip,
        // not a hand-off), and it handed the chain to this step ALONE
        // (out-degree 1 — a fan-out is out-degree ≥ 2, ready).
        let baton = plist.is_some_and(|ps| {
            ps.len() == 1
                && !ps[0].1
                && terminal_of(meta, &ps[0].2) == Terminal::Done
                && out_degree.get(ps[0].0.as_str()) == Some(&1)
        });
        step.state = derive_state(
            meta,
            step.archived,
            &step.status,
            satisfied,
            has_preds,
            baton,
        );
    }
}

/// Everything the engine needs to decide a step's edge: the task's terminal
/// category, if any. A done-category column is `Done`; the off-board keys are
/// `OffBoard`; anything else is `Live` (archival is not a status — the
/// derived head already reads past archived steps, and the engine does too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    Live,
    Done,
    OffBoard,
}

/// Which terminal category a status names, per the board's own StatusMeta —
/// the one definition (done_keys + OFF_BOARD_STATUSES), split the way the
/// engine needs it because a done step ADVANCES its chain and a failed one
/// PAUSES it.
pub fn terminal_of(meta: &StatusMeta, status: &str) -> Terminal {
    if OFF_BOARD_STATUSES.contains(&status) {
        Terminal::OffBoard
    } else if meta.done_keys.iter().any(|k| k == status) {
        Terminal::Done
    } else {
        Terminal::Live
    }
}

/// A ready candidate row: task id, status, assignees — everything deciding
/// whether a fan-out notification goes out and to whom.
type ReadyRow = (String, String, serde_json::Value);

/// The successors of one task's step that just became ready: steps with an
/// edge FROM the fired task, whose EVERY predecessor is now satisfied (done
/// or archived) and which are themselves live (not archived, not terminal —
/// an already-done successor has no turn to hand to). The fired step being
/// one of the successors' preds is the structure of the query: fan-out only
/// fires the edges the completed step owns.
///
/// Two indexed reads, deliberately not one clever query: the successors,
/// then per successor a count of its still-open OTHER predecessors. Small
/// graphs; a shape the next reader can follow beats a join nobody can.
async fn newly_ready_of(
    pg: &PgPool,
    meta: &StatusMeta,
    task_id: &str,
) -> Result<Vec<ReadyRow>, sqlx::Error> {
    let succ_rows: Vec<(String, String, serde_json::Value, bool)> = sqlx::query_as(
        "select distinct ts.task_id::text, t.status, t.assignees, \
                (t.archived_at is not null) \
         from task_workchain_edges e \
         join task_workchain_steps fs on fs.id = e.from_step \
         join task_workchain_steps ts on ts.id = e.to_step \
         join tasks t on t.id = ts.task_id \
         where fs.task_id = $1::uuid",
    )
    .bind(task_id)
    .fetch_all(pg)
    .await?;
    let mut out = Vec::new();
    for (succ_id, status, assignees, archived) in succ_rows {
        // The successor must be live to receive a turn.
        if archived || terminal_of(meta, &status) != Terminal::Live {
            continue;
        }
        // Every OTHER predecessor of this successor must be satisfied. The
        // fired task's own edge is the trigger; the rest decide the join.
        // A pred is open when its ticket is unarchived and its column is
        // neither a done key (work completed) nor an off-board key — a
        // failed pred is NOT satisfaction, the chain paused instead.
        let pending: Option<(i64,)> = sqlx::query_as(
            "select count(*)::bigint \
             from task_workchain_edges e \
             join task_workchain_steps fs on fs.id = e.from_step \
             join tasks ft on ft.id = fs.task_id \
             where e.to_step in ( \
                 select id from task_workchain_steps where task_id = $1::uuid \
             ) \
               and ft.id <> $2::uuid \
               and ft.archived_at is null \
               and ft.status <> all($3::text[])",
        )
        .bind(&succ_id)
        .bind(task_id)
        .bind(done_keys_of(meta))
        .fetch_optional(pg)
        .await?;
        // count(*) always returns a row.
        let Some((pending,)) = pending else {
            continue;
        };
        if pending == 0 {
            out.push((succ_id, status, assignees));
        }
    }
    Ok(out)
}

/// The board's done keys plus the off-board keys as a text[] bind — the
/// predicate that says a predecessor's work is FINISHED-or-off-the-table
/// for join purposes is exactly done_keys ∪ OFF_BOARD_STATUSES... except
/// the engine treats off-board as pause, not completion. So this bind is
/// done_keys only, and the pause path answers the failed-pred case before
/// any join can fire on one. (A done pred satisfies; an off-board pred
/// leaves the successor waiting under the paused chain.)
fn done_keys_of(meta: &StatusMeta) -> Vec<String> {
    meta.done_keys.clone()
}

/// Fire the engine after a task's status column was written. `board_id` and
/// the task's CURRENT status name the state to derive from — the caller
/// passes what update_task just wrote (next_status), so the DB read below
/// agrees with the write that fired it.
///
/// DETACHED BY DESIGN: every notification is fire-and-forget through the one
/// writer; a paused write or a failed row never costs the ticket write it
/// rode in on. The reads below are awaited (they must be: the notification's
/// content derives from them) — indexed lookups on a chain-less ticket, a
/// couple of graph reads on one that has a chain.
pub async fn advance_workchains(
    pg: &PgPool,
    notify: &NotifyDeps,
    board_id: &str,
    task_id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    let meta = status_meta(pg, board_id).await?;
    let category = terminal_of(&meta, status);
    // The chain this task sits in (one chain per task — the unique index).
    let chain: Option<(String, String, Option<String>, bool)> = sqlx::query_as(
        "select w.id::text, w.name, w.created_by, w.paused \
         from task_workchain_steps s \
         join task_workchains w on w.id = s.workchain_id \
         where s.task_id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    let Some((chain_id, chain_name, chain_creator, chain_paused)) = chain else {
        return Ok(());
    };
    match category {
        // Done: the fan-out. Every successor whose predecessors are ALL now
        // satisfied becomes ready at once; its HUMAN assignees are told it
        // is their turn. An agent successor hears nothing — its visibility
        // is the heartbeat serving the ready head.
        Terminal::Done => {
            let ready = newly_ready_of(pg, &meta, task_id).await?;
            // Unset seam = unwired boot, not a panic: skip the notification
            // rather than blow the request off the rails (the seam is set in
            // register_all; this arm is the safety net, same shape as the
            // attribution ladder's rungs).
            let Some(f) = GET_TASK.get() else {
                tracing::warn!("[workchains] GET_TASK seam unset — turn notification skipped");
                return Ok(());
            };
            for (succ_id, _, _) in ready {
                let Some(t) = f(pg.clone(), succ_id.clone()).await? else {
                    continue;
                };
                let humans = talaria_tasks_types::human_assignee_ids(&t.assignees);
                if humans.is_empty() {
                    continue;
                }
                let subject = match t.ticket_ref.as_deref() {
                    Some(r) => format!("{r} - {}", t.title),
                    None => t.title.clone(),
                };
                let href = format!("/boards/{}/{}", t.board_id, t.id);
                for user_id in &humans {
                    let input = NotificationInput {
                        kind: "workchain_turn",
                        title: &format!("It's your turn: {subject}"),
                        body: None,
                        href: Some(&href),
                    };
                    if let Err(e) = talaria_notify::add_notification(notify, user_id, &input).await
                    {
                        tracing::error!("[workchains] turn notification for {user_id} failed: {e}");
                    }
                }
            }
        }
        // Off-board (failed/cancelled): the chain stops advancing until a
        // human resumes it. Pause and tell the chain's creator. Unpausing
        // is a human PATCH — it re-derives the ready heads on the next read
        // and auto-advances nothing, exactly like the chain's own reader.
        // (v1 semantics, kept: the WHOLE chain pauses. Whether a failure
        // should quarantine only its downstream subtree is TALA-35's open
        // question for triage — this write stays whole-chain until triage
        // answers it, and nothing stored depends on the answer.)
        Terminal::OffBoard => {
            if !chain_paused {
                sqlx::query(
                    "update task_workchains set paused = true, updated_at = now() \
                     where id = $1::uuid",
                )
                .bind(&chain_id)
                .execute(pg)
                .await?;
            }
            let Some(creator) = chain_creator else {
                return Ok(());
            };
            let Some(f) = GET_TASK.get() else {
                tracing::warn!("[workchains] GET_TASK seam unset — pause notification skipped");
                return Ok(());
            };
            let Some(t) = f(pg.clone(), task_id.to_string()).await? else {
                return Ok(());
            };
            let subject = match t.ticket_ref.as_deref() {
                Some(r) => format!("{r} {}", t.title),
                None => t.title.clone(),
            };
            // created_by is the human-readable attribution the POST stored
            // (email, else name, else 'user') — resolve it to a users.id the
            // notification writer can file a row against, the same email →
            // id lookup notify_task_users uses for its actor.
            let creator_id: Option<(String,)> =
                sqlx::query_as("select id::text from users where lower(email) = $1 limit 1")
                    .bind(creator.to_lowercase())
                    .fetch_optional(pg)
                    .await?;
            let Some((creator_id,)) = creator_id else {
                return Ok(());
            };
            let input = NotificationInput {
                kind: "workchain_paused",
                title: &format!("Workchain paused: {chain_name} - {subject}"),
                body: None,
                href: Some(&format!("/boards/{}/{}", t.board_id, t.id)),
            };
            if let Err(e) = talaria_notify::add_notification(notify, &creator_id, &input).await {
                tracing::error!("[workchains] paused notification for {creator} failed: {e}");
            }
        }
        Terminal::Live => {}
    }
    Ok(())
}

// ── The heartbeat's ordering guarantee ───────────────────────────────────────

/// The workchain answer for one servable-candidate ticket, derived the same
/// instant the heartbeat reads it: `Blocked` when a predecessor of its chain
/// step is still live (the ticket must NOT be served, whatever its own
/// column says), `Ready` when the step IS servable now — no predecessors
/// (an entry point) or every predecessor done — flagged so the harness can
/// prioritize chain work, `Free` when the ticket belongs to no chain (the
/// ordinary heartbeat shape, unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Free,
    Ready,
    Blocked,
}

impl Readiness {
    /// The wire field rides only on chain tickets: a `Free` ticket keeps the
    /// feed item shape it always had, a `Ready` one carries
    /// `workchainReady: true` — the minimum-churn shape the harnesses read.
    pub fn ready_flag(self) -> Option<bool> {
        match self {
            Readiness::Free => None,
            Readiness::Ready => Some(true),
            Readiness::Blocked => None,
        }
    }
}

/// Chain readiness for a batch of candidate tickets, ONE query for the whole
/// heartbeat. The query carries NO rule of its own — it fetches the chains'
/// steps and edges and the terminal predicate is derived in Rust, through the
/// one definition the board read uses. Spelling it in SQL is not possible
/// without a second copy of the rule: done columns are per-board, and a board
/// that never customized has NO board_statuses rows at all — its 'done'
/// column is a virtual default only StatusMeta knows.
///
/// A candidate is Blocked when any predecessor of its step is still live
/// (not archived, not done-terminal). It is Ready when it has NO
/// predecessors or all of them are satisfied — a branched chain legitimately
/// serves several ready steps at once (the fan-out is the point). A PAUSED
/// chain serves its ready steps exactly as an unpaused one — pause is the
/// creator-facing signal that the chain stopped advancing, not a gate on
/// work already at the front.
///
/// The map carries an entry for every LIVE step of the chains touched
/// (Ready or Blocked); a candidate absent from it belongs to no chain and
/// stays Free.
pub async fn chain_readiness(
    pg: &PgPool,
    metas: &HashMap<String, StatusMeta>,
    candidate_ids: &[String],
) -> Result<HashMap<String, Readiness>, sqlx::Error> {
    if candidate_ids.is_empty() {
        return Ok(HashMap::new());
    }
    // Every step of every chain a candidate belongs to, plus the chains'
    // edges. The chain's board rides along so the terminal predicate comes
    // from that board's meta — a chain's steps are all its board's tickets
    // (cross-board adds are refused), so ONE meta answers every step.
    let rows: Vec<(String, String, String, String, bool)> = sqlx::query_as(
        "select w.id::text, w.board_id::text, s.task_id::text, t.status, \
                (t.archived_at is not null) \
         from task_workchain_steps s \
         join task_workchains w on w.id = s.workchain_id \
         join tasks t on t.id = s.task_id \
         where s.workchain_id in ( \
             select workchain_id from task_workchain_steps where task_id = any($1::uuid[]) \
         )",
    )
    .bind(candidate_ids)
    .fetch_all(pg)
    .await?;
    let edge_rows: Vec<(String, String, String)> = sqlx::query_as(
        "select e.workchain_id::text, ft.task_id::text, ts.task_id::text \
         from task_workchain_edges e \
         join task_workchain_steps fs on fs.id = e.from_step \
         join task_workchain_steps ts on ts.id = e.to_step \
         where e.workchain_id in ( \
             select workchain_id from task_workchain_steps where task_id = any($1::uuid[]) \
         )",
    )
    .bind(candidate_ids)
    .fetch_all(pg)
    .await?;
    // Per chain: the step table (with board id) and the predecessor lists.
    let mut steps_by_chain: HashMap<String, Vec<(String, String, String, bool)>> = HashMap::new(); // chain → (task, board, status, archived)
    for (chain_id, board_id, task_id, status, archived) in &rows {
        steps_by_chain.entry(chain_id.clone()).or_default().push((
            task_id.clone(),
            board_id.clone(),
            status.clone(),
            *archived,
        ));
    }
    let mut preds_by_chain: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (chain_id, from, to) in edge_rows {
        preds_by_chain.entry(chain_id).or_default().push((from, to));
    }
    // One derive per chain: the map carries an entry for every LIVE step
    // (Ready or Blocked) of the chains touched.
    let mut out: HashMap<String, Readiness> = HashMap::new();
    for (chain_id, steps) in &steps_by_chain {
        // All of a chain's steps sit on its board (cross-board adds are
        // refused), so the first board id answers the whole chain.
        let Some((_, board_id, _, _)) = steps.first() else {
            continue;
        };
        let Some(meta) = metas.get(board_id) else {
            continue;
        };
        let edges = preds_by_chain
            .get(chain_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let mut preds: HashMap<&str, Vec<(&str, bool, &str)>> = HashMap::new();
        for (from, to) in edges {
            let Some(f) = steps.iter().find(|(id, _, _, _)| id == from) else {
                continue;
            };
            preds
                .entry(to.as_str())
                .or_default()
                .push((f.0.as_str(), f.3, f.2.as_str()));
        }
        for (task_id, _, status, archived) in steps {
            if *archived || terminal_of(meta, status) != Terminal::Live {
                continue;
            }
            let plist = preds.get(task_id.as_str());
            let all_satisfied = plist
                .map(|ps| {
                    ps.iter()
                        .all(|(_, archived, status)| pred_satisfied(*archived, status, meta))
                })
                .unwrap_or(true);
            let has_preds = plist.map(|p| !p.is_empty()).unwrap_or(false);
            out.insert(
                task_id.clone(),
                if !has_preds || all_satisfied {
                    Readiness::Ready
                } else {
                    Readiness::Blocked
                },
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(task_id: &str, status: &str, archived: bool) -> WorkchainStep {
        WorkchainStep {
            task_id: task_id.into(),
            position: 0,
            state: "blocked",
            ticket_ref: None,
            title: String::new(),
            assignees: vec![],
            effort: None,
            due_date: None,
            status: status.into(),
            archived,
            x: None,
            y: None,
        }
    }

    /// The meta a default-shaped board resolves (DEFAULTS → done_keys
    /// ['done']); the derive under test consumes StatusMeta, and every
    /// surface shares the one terminal() definition.
    fn meta(done_keys: &[&str]) -> StatusMeta {
        StatusMeta {
            keys: vec![],
            agent_start_keys: vec![],
            review_key: None,
            review_keys: vec![],
            done_keys: done_keys.iter().map(|k| k.to_string()).collect(),
            default_key: None,
            assigned_key: None,
            pickup_keys: vec![],
            working_keys: vec![],
            active_key: None,
        }
    }

    fn wire(edges: &[(&str, &str)]) -> Vec<(String, String)> {
        edges
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn the_first_live_step_is_the_head_and_the_rest_wait() {
        let meta = meta(&["done"]);
        let mut steps = vec![
            step("a", "done", false),
            step("b", "inbox", false),
            step("c", "in_progress", false),
        ];
        derive_states(&meta, &mut steps, &wire(&[("a", "b"), ("b", "c")]));
        assert_eq!(steps[0].state, "done");
        assert_eq!(steps[1].state, "head");
        assert_eq!(steps[2].state, "blocked");
    }

    #[test]
    fn archived_steps_are_read_past_never_heading_the_chain() {
        let meta = meta(&["done"]);
        let mut steps = vec![
            step("a", "done", false),
            step("retired", "inbox", true),
            step("c", "inbox", false),
        ];
        derive_states(
            &meta,
            &mut steps,
            &wire(&[("a", "retired"), ("retired", "c")]),
        );
        assert_eq!(steps[1].state, "archived");
        // The chain reads past the archived step: its successor's only
        // predecessor is archived → satisfied → the step is served.
        assert_eq!(steps[2].state, "ready");
    }

    #[test]
    fn a_custom_done_key_is_terminal_like_the_default_one() {
        // done_keys is whatever the board's done-category columns are — a
        // board that renamed its done column to 'shipped' answers the same
        // terminal() the shipped key spells.
        let meta = meta(&["shipped"]);
        let mut steps = vec![step("a", "shipped", false), step("b", "inbox", false)];
        derive_states(&meta, &mut steps, &wire(&[("a", "b")]));
        assert_eq!(steps[0].state, "done");
        assert_eq!(steps[1].state, "head");
    }

    #[test]
    fn an_all_done_chain_has_no_head() {
        let meta = meta(&["done"]);
        let mut steps = vec![step("a", "done", false), step("b", "done", false)];
        derive_states(&meta, &mut steps, &wire(&[("a", "b")]));
        assert_eq!(steps[0].state, "done");
        assert_eq!(steps[1].state, "done");
    }

    #[test]
    fn terminal_of_splits_done_from_off_board() {
        let meta = meta(&["done", "shipped"]);
        assert_eq!(terminal_of(&meta, "done"), Terminal::Done);
        assert_eq!(terminal_of(&meta, "shipped"), Terminal::Done);
        assert_eq!(terminal_of(&meta, "failed"), Terminal::OffBoard);
        assert_eq!(terminal_of(&meta, "cancelled"), Terminal::OffBoard);
        assert_eq!(terminal_of(&meta, "inbox"), Terminal::Live);
        assert_eq!(terminal_of(&meta, "blocked"), Terminal::Live);
    }

    #[test]
    fn an_off_board_key_is_off_board_even_when_a_board_names_a_column_that_way() {
        // OFF_BOARD_STATUSES wins over done_keys by the check order — the
        // engine pauses on failed/cancelled whatever the board's columns
        // say, matching StatusMeta::terminal's own precedence.
        let meta = meta(&["failed"]);
        assert_eq!(terminal_of(&meta, "failed"), Terminal::OffBoard);
        assert!(meta.terminal("failed"));
    }

    // ── TALA-35: the graph derive ───────────────────────────────────────────

    #[test]
    fn a_fan_out_serves_every_branch_at_once() {
        // A → B, A → C: when A completes, B AND C are both served — the
        // fan-out is the point of the graph. B derives ready (its pred is
        // done); C the same; neither blocks the other.
        let meta = meta(&["done"]);
        let mut steps = vec![
            step("a", "done", false),
            step("b", "inbox", false),
            step("c", "in_progress", false),
        ];
        derive_states(&meta, &mut steps, &wire(&[("a", "b"), ("a", "c")]));
        assert_eq!(steps[0].state, "done");
        assert_eq!(steps[1].state, "ready");
        assert_eq!(steps[2].state, "ready");
    }

    #[test]
    fn an_and_join_waits_for_its_last_open_predecessor() {
        // B has preds A and C; A is done, C is live → B is blocked. When C
        // completes the same derive serves B: the join fired.
        let meta = meta(&["done"]);
        let mut steps = vec![
            step("a", "done", false),
            step("b", "inbox", false),
            step("c", "in_progress", false),
        ];
        derive_states(&meta, &mut steps, &wire(&[("a", "b"), ("c", "b")]));
        assert_eq!(steps[1].state, "blocked");
        steps[2].status = "done".into();
        derive_states(&meta, &mut steps, &wire(&[("a", "b"), ("c", "b")]));
        assert_eq!(steps[1].state, "ready");
    }

    #[test]
    fn a_failed_predecessor_does_not_satisfy_the_join() {
        // The work did not complete: a failed pred leaves the join waiting
        // (the engine has already paused the chain and told its creator).
        let meta = meta(&["done"]);
        let mut steps = vec![step("a", "failed", false), step("b", "inbox", false)];
        derive_states(&meta, &mut steps, &wire(&[("a", "b")]));
        assert_eq!(steps[1].state, "blocked");
    }

    #[test]
    fn two_entry_points_both_read_head() {
        // Disconnected nodes in one chain: each with no preds is an entry
        // point, and each is served — the graph says so, not the order.
        let meta = meta(&["done"]);
        let mut steps = vec![step("a", "inbox", false), step("b", "inbox", false)];
        derive_states(&meta, &mut steps, &[]);
        assert_eq!(steps[0].state, "head");
        assert_eq!(steps[1].state, "head");
    }

    #[test]
    fn a_diamond_serves_only_when_the_join_closes() {
        // A → B, A → C, B → D, C → D: D waits for BOTH branches. B done,
        // C live → D blocked; C done → D ready.
        let meta = meta(&["done"]);
        let edges = wire(&[("a", "b"), ("a", "c"), ("b", "d"), ("c", "d")]);
        let mut steps = vec![
            step("a", "done", false),
            step("b", "done", false),
            step("c", "in_progress", false),
            step("d", "inbox", false),
        ];
        derive_states(&meta, &mut steps, &edges);
        assert_eq!(steps[3].state, "blocked");
        steps[2].status = "done".into();
        derive_states(&meta, &mut steps, &edges);
        assert_eq!(steps[3].state, "ready");
    }
}

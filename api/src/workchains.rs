// Board workchains — ordered pipelines of tickets (A -> B -> C) with
// human/agent handoffs (TALA-30).
//
// The step state is derived, never stored: a step is 'done' when its task
// sits in a done-category column (the one definition, StatusMeta::terminal),
// the first non-done, non-archived step is the chain's 'head', and every
// later step is 'waiting'. Archived tickets never count as head — a retired
// ticket is not work in flight, and reading past it is the answer the
// chain's own reader needs.
//
// ── The handoff engine ────────────────────────────────────────────────────────
//
// advance_workchains is the write-side half of the same derived state: when a
// task write lands a step's ticket in a terminal column, every chain the task
// belongs to is re-derived FROM THE DATABASE and the new head's assignees
// decide what happens next. Humans are TOLD (a workchain_turn row through
// the one notification writer); agents are not — their visibility is the
// heartbeat, which serves a ready head and hides a blocked step, so an agent
// cannot read its chain position as an invitation to start early. An
// off-board terminal (failed/cancelled) pauses the chain and tells its
// creator, because a chain whose step failed is not a chain that advances.
//
// The engine fires on the ordinary status-write path only — the one door
// update_task is — so a human sign-off (or a reviewer's approve) is the only
// trigger. Agents cannot land a terminal column (the MCP guardrail), which
// makes human approval the sole way a chain advances: nothing else calls
// this, on purpose.
//
// IDEMPOTENT BY CONSTRUCTION: no chain state lives in memory. Each firing
// re-reads the chain from the DB, so a retried status write re-derives the
// same head and files nothing new (the head only changes when the DB does).
// A head that is already terminal-derived cannot be re-notified — the
// notification rides the TRANSITION, and a transition that didn't happen
// derives the same answer twice, quietly.

use crate::notify::{NotificationInput, NotifyDeps};
use crate::statuses::{OFF_BOARD_STATUSES, StatusMeta, status_meta};
use sqlx::PgPool;
use std::collections::HashMap;

/// A workchain on the wire: id, boardId, name, createdBy, paused, position,
/// createdAt, updatedAt, then its steps in position order with each step's
/// task summary and the derived state.
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
}

/// One step: its task's summary — the fields a chain card renders (ref,
/// title, assignees, effort, due, status, archived) — plus `state`, the
/// derived done/head/waiting. Deliberately not the full Task: a chain is a
/// reading surface; the card link opens the ticket.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkchainStep {
    pub task_id: String,
    pub position: i32,
    /// 'done' | 'head' | 'waiting' | 'archived' — derived per read, never
    /// stored.
    pub state: &'static str,
    pub ticket_ref: Option<String>,
    pub title: String,
    pub assignees: Vec<String>,
    pub effort: Option<String>,
    pub due_date: Option<String>,
    pub status: String,
    pub archived: bool,
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
);

fn iso(ms: i64) -> String {
    crate::agent_auth::epoch_ms_to_iso(ms)
}

/// The one derive, split out so the state rules are testable against a
/// synthetic list. Steps arrive in the order the chain reads (position, then
/// created_at — the list query's tiebreak); the first non-archived step
/// whose task is not terminal is the head, everything after stays 'waiting'.
fn derive_states(meta: &StatusMeta, steps: &mut [WorkchainStep]) {
    let mut head_seen = false;
    for step in steps.iter_mut() {
        // Archived tickets never count as head — they keep their summary
        // and their 'archived' state, and the chain reads past them.
        if step.archived {
            step.state = "archived";
        } else if meta.terminal(&step.status) {
            step.state = "done";
        } else if head_seen {
            step.state = "waiting";
        } else {
            step.state = "head";
            head_seen = true;
        }
    }
}

/// A step from its row: every field is carried, the state is a placeholder —
/// derive_states assigns it in the chain read, from the same StatusMeta.
fn step_of(meta: &StatusMeta, row: StepRow) -> WorkchainStep {
    let (_, task_id, position, ticket_ref, title, assignees, effort, due_ms, status, archived_ms) =
        row;
    let archived = archived_ms.is_some();
    WorkchainStep {
        task_id,
        position,
        // Placeholder: derive_states assigns every step's state in the
        // chain read below, from the same StatusMeta.
        state: "waiting",
        ticket_ref,
        title,
        assignees: crate::tasks::json_strings(&assignees),
        effort,
        due_date: due_ms.map(iso),
        status,
        archived,
    }
}

/// The board's workchains in position order, each with its steps joined to
/// task summaries and the derived done/head/waiting state. Two queries: the
/// chains, then every chain's steps in one pass grouped by workchain id.
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
                (trunc(extract(epoch from t.archived_at) * 1000))::bigint \
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
    let meta = status_meta(pg, board_id).await?;
    let mut by_chain: HashMap<String, Vec<WorkchainStep>> = HashMap::new();
    for row in steps {
        let chain = row.0.clone();
        by_chain.entry(chain).or_default().push(step_of(&meta, row));
    }
    Ok(chains
        .into_iter()
        .map(
            |(id, board_id, name, created_by, paused, position, created_ms, updated_ms)| {
                let mut steps = by_chain.remove(&id).unwrap_or_default();
                derive_states(&meta, &mut steps);
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
                }
            },
        )
        .collect())
}

// ── The handoff engine ───────────────────────────────────────────────────────

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

/// A head candidate row, in select order: task id, status, assignees, title,
/// ticket ref (None when the board has no prefix), chain id, chain name,
/// chain creator, chain paused.
type HeadRow = (
    String,
    String,
    serde_json::Value,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    bool,
);

/// Read the chain the task is a step of — its id, name, creator and paused
/// flag — or None when the task belongs to no chain (the common case; most
/// writes are ordinary tickets and cost one indexed lookup).
async fn chain_of(pg: &PgPool, task_id: &str) -> Result<Option<HeadRow>, sqlx::Error> {
    let row: Option<HeadRow> = sqlx::query_as(
        "select t.id::text, t.status, t.assignees, t.title, \
                case when t.ticket_no is not null \
                     then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end, \
                w.id::text, w.name, w.created_by, w.paused \
         from task_workchain_steps s \
         join task_workchains w on w.id = s.workchain_id \
         join tasks t on t.id = s.task_id \
         join boards b on b.id = t.board_id \
         where s.task_id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    Ok(row)
}

/// The chain's head, derived the same way list_workchains derives it: the
/// first step, in position order, whose task is not archived and not
/// terminal. None when the chain has no live work left. Returns the head's
/// task id, status and assignees (the engine's whole decision input).
async fn head_of(
    pg: &PgPool,
    meta: &StatusMeta,
    chain_id: &str,
) -> Result<Option<(String, String, serde_json::Value)>, sqlx::Error> {
    let rows: Vec<(String, String, serde_json::Value, Option<i64>)> = sqlx::query_as(
        "select t.id::text, t.status, t.assignees, \
                (trunc(extract(epoch from t.archived_at) * 1000))::bigint \
         from task_workchain_steps s \
         join tasks t on t.id = s.task_id \
         where s.workchain_id = $1::uuid \
         order by s.position, s.created_at",
    )
    .bind(chain_id)
    .fetch_all(pg)
    .await?;
    for (id, status, assignees, archived_ms) in rows {
        if archived_ms.is_some() {
            continue;
        }
        if terminal_of(meta, &status) == Terminal::Live {
            return Ok(Some((id, status, assignees)));
        }
    }
    Ok(None)
}

/// Fire the engine after a task's status column was written. `board_id` and
/// the task's CURRENT status name the state to derive from — the caller
/// passes what update_task just wrote (next_status), so the DB read below
/// agrees with the write that fired it.
///
/// DETACHED BY DESIGN: every notification is fire-and-forget through the one
/// writer; a paused write or a failed row never costs the ticket write it
/// rode in on. The chain read and the status_meta read are awaited (they
/// must be: the notification's content derives from them) — one indexed
/// lookup on a chain-less ticket, a chain read plus a column read on one
/// that has a chain.
pub async fn advance_workchains(
    pg: &PgPool,
    notify: &NotifyDeps,
    board_id: &str,
    task_id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    let meta = status_meta(pg, board_id).await?;
    let category = terminal_of(&meta, status);
    let Some((_, _, _, _, _, chain_id, chain_name, chain_creator, chain_paused)) =
        chain_of(pg, task_id).await?
    else {
        return Ok(());
    };
    match category {
        // Done: the chain advanced past this step. Tell the new head's
        // HUMAN assignees it is their turn; an agent head hears nothing —
        // its visibility is the heartbeat serving the ready head.
        Terminal::Done => {
            let Some((head_id, _, head_assignees)) = head_of(pg, &meta, &chain_id).await? else {
                return Ok(());
            };
            let humans =
                crate::tasks::human_assignee_ids(&crate::tasks::json_strings(&head_assignees));
            if humans.is_empty() {
                return Ok(());
            }
            let Some(t) = crate::tasks::get_task(pg, &head_id).await? else {
                return Ok(());
            };
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
                if let Err(e) = crate::notify::add_notification(notify, user_id, &input).await {
                    tracing::error!("[workchains] turn notification for {user_id} failed: {e}");
                }
            }
        }
        // Off-board (failed/cancelled): the chain stops advancing until a
        // human resumes it. Pause and tell the chain's creator. Unpausing
        // is a human PATCH — it re-derives the head on the next read and
        // auto-advances nothing, exactly like the chain's own reader.
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
            let Some(t) = crate::tasks::get_task(pg, task_id).await? else {
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
            if let Err(e) = crate::notify::add_notification(notify, &creator_id, &input).await {
                tracing::error!("[workchains] paused notification for {creator} failed: {e}");
            }
        }
        Terminal::Live => {}
    }
    Ok(())
}

// ── The heartbeat's ordering guarantee ───────────────────────────────────────

/// The workchain answer for one servable-candidate ticket, derived the same
/// instant the heartbeat reads it: `Blocked` when an earlier step of its
/// chain is still live (the ticket must NOT be served, whatever its own
/// column says), `Ready` when it IS its chain's head (serve it, flagged so
/// the harness can prioritize chain work), `Free` when the ticket belongs to
/// no chain (the ordinary heartbeat shape, unchanged).
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
/// steps (position order, the same tiebreak the chain read uses) and the
/// terminal predicate is derived in Rust, through the one definition the
/// board read uses. Spelling it in SQL is not possible without a second
/// copy of the rule: done columns are per-board, and a board that never
/// customized has NO board_statuses rows at all — its 'done' column is a
/// virtual default only StatusMeta knows.
///
/// A candidate is Blocked when any EARLIER step of its chain is still live
/// (not archived, not terminal). It is Ready when it IS that first live
/// step. A PAUSED chain serves its head exactly as an unpaused one — pause
/// is the creator-facing signal that the chain stopped advancing, not a
/// gate on the work already at the front.
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
    // Every step of every chain a candidate belongs to, in the order the
    // chain read walks them: position, then created_at. The chain's board
    // rides along so the terminal predicate comes from that board's meta —
    // a chain's steps are all its board's tickets (cross-board adds are
    // refused), so ONE meta answers every step.
    let rows: Vec<(String, String, String, String, bool)> = sqlx::query_as(
        "select w.id::text, w.board_id::text, s.task_id::text, t.status, \
                (t.archived_at is not null) \
         from task_workchain_steps s \
         join task_workchains w on w.id = s.workchain_id \
         join tasks t on t.id = s.task_id \
         where s.workchain_id in ( \
             select workchain_id from task_workchain_steps where task_id = any($1::uuid[]) \
         ) \
         order by w.id, s.position, s.created_at",
    )
    .bind(candidate_ids)
    .fetch_all(pg)
    .await?;
    let mut by_chain: HashMap<(&str, &str), Vec<(&str, &str, bool)>> = HashMap::new();
    for (chain_id, board_id, task_id, status, archived) in &rows {
        by_chain
            .entry((chain_id.as_str(), board_id.as_str()))
            .or_default()
            .push((task_id.as_str(), status.as_str(), *archived));
    }
    // One walk per chain: the first live step is the head (Ready); every
    // live step behind it is Blocked. The map ends up carrying an entry
    // for every LIVE step of every chain touched — a candidate absent from
    // it belongs to no chain and stays Free.
    let mut out: HashMap<String, Readiness> = HashMap::new();
    for ((_, board_id), steps) in &by_chain {
        let Some(meta) = metas.get(*board_id) else {
            continue;
        };
        let mut head_seen = false;
        for (task_id, status, archived) in steps {
            if *archived || terminal_of(meta, status) != Terminal::Live {
                continue;
            }
            out.insert(
                task_id.to_string(),
                if head_seen {
                    Readiness::Blocked
                } else {
                    Readiness::Ready
                },
            );
            head_seen = true;
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
            state: "waiting",
            ticket_ref: None,
            title: String::new(),
            assignees: vec![],
            effort: None,
            due_date: None,
            status: status.into(),
            archived,
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

    #[test]
    fn the_first_live_step_is_the_head_and_the_rest_wait() {
        let meta = meta(&["done"]);
        let mut steps = vec![
            step("a", "done", false),
            step("b", "inbox", false),
            step("c", "in_progress", false),
        ];
        derive_states(&meta, &mut steps);
        assert_eq!(steps[0].state, "done");
        assert_eq!(steps[1].state, "head");
        assert_eq!(steps[2].state, "waiting");
    }

    #[test]
    fn archived_steps_are_read_past_never_heading_the_chain() {
        let meta = meta(&["done"]);
        let mut steps = vec![
            step("a", "done", false),
            step("retired", "inbox", true),
            step("c", "inbox", false),
        ];
        derive_states(&meta, &mut steps);
        assert_eq!(steps[1].state, "archived");
        assert_eq!(steps[2].state, "head");
    }

    #[test]
    fn a_custom_done_key_is_terminal_like_the_default_one() {
        // done_keys is whatever the board's done-category columns are — a
        // board that renamed its done column to 'shipped' answers the same
        // terminal() the shipped key spells.
        let meta = meta(&["shipped"]);
        let mut steps = vec![step("a", "shipped", false), step("b", "inbox", false)];
        derive_states(&meta, &mut steps);
        assert_eq!(steps[0].state, "done");
        assert_eq!(steps[1].state, "head");
    }

    #[test]
    fn an_all_done_chain_has_no_head() {
        let meta = meta(&["done"]);
        let mut steps = vec![step("a", "done", false), step("b", "done", false)];
        derive_states(&meta, &mut steps);
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
}

// Board workchains — ordered pipelines of tickets (A -> B -> C) with
// human/agent handoffs (TALA-30, data model + CRUD; no UI yet).
//
// The step state is derived, never stored: a step is 'done' when its task
// sits in a done-category column (the one definition, StatusMeta::terminal),
// the first non-done, non-archived step is the chain's 'head', and every
// later step is 'waiting'. Archived tickets never count as head — a retired
// ticket is not work in flight, and reading past it is the answer the
// chain's own reader needs.

use crate::statuses::{StatusMeta, status_meta};
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
}

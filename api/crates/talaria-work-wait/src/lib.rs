// Work wait — the visible queue when packing refuses a workbench session.
//
// Dispatch used to stand down silently: the 60s sweep re-asked, the ticket
// sat in a pickup column with no session, and the board looked idle. This
// table is that wait, named. One row per ticket. Cleared the moment a
// session actually starts. Not a job scheduler — the sweep is still the
// retry — just the thing the UI can show.

use sqlx::PgPool;

pub struct Wait {
    pub agent_model: String,
    pub reason: String,
    pub queued_ms: i64,
    pub position: i64,
}

/// Upsert the wait. `queued_at` stays the first time we queued this ticket
/// so position is stable across sweep re-asks.
pub async fn mark_waiting(pg: &PgPool, task_id: &str, agent_model: &str, reason: &str) {
    let _ = sqlx::query(
        "insert into work_wait (task_id, agent_model, reason) \
         values ($1::uuid, $2, $3) \
         on conflict (task_id) do update set \
           agent_model = excluded.agent_model, \
           reason = excluded.reason, \
           updated_at = now()",
    )
    .bind(task_id)
    .bind(agent_model)
    .bind(reason)
    .execute(pg)
    .await;
}

pub async fn clear_waiting(pg: &PgPool, task_id: &str) {
    let _ = sqlx::query("delete from work_wait where task_id = $1::uuid")
        .bind(task_id)
        .execute(pg)
        .await;
}

pub async fn for_task(pg: &PgPool, task_id: &str) -> Option<Wait> {
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "select agent_model, reason, \
                (trunc(extract(epoch from queued_at) * 1000))::bigint \
         from work_wait where task_id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    let (agent_model, reason, queued_ms) = row?;
    let position = position_of(pg, &agent_model, queued_ms).await;
    Some(Wait {
        agent_model,
        reason,
        queued_ms,
        position,
    })
}

pub async fn for_board(pg: &PgPool, board_id: &str) -> Vec<(String, Wait)> {
    let rows: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "select w.task_id::text, w.agent_model, w.reason, \
                (trunc(extract(epoch from w.queued_at) * 1000))::bigint, \
                rank() over (partition by w.agent_model order by w.queued_at, w.task_id) \
         from work_wait w \
         join tasks t on t.id = w.task_id \
         where t.board_id = $1::uuid \
         order by w.queued_at",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(task_id, agent_model, reason, queued_ms, position)| {
            (
                task_id,
                Wait {
                    agent_model,
                    reason,
                    queued_ms,
                    position,
                },
            )
        })
        .collect()
}

async fn position_of(pg: &PgPool, agent_model: &str, queued_ms: i64) -> i64 {
    let n: i64 = sqlx::query_scalar(
        "select count(*) from work_wait \
         where agent_model = $1 \
           and (trunc(extract(epoch from queued_at) * 1000))::bigint <= $2",
    )
    .bind(agent_model)
    .bind(queued_ms)
    .fetch_one(pg)
    .await
    .unwrap_or(1);
    n.max(1)
}

/// What a queued ticket is waiting BEHIND: the tickets its agent has live
/// work sessions on, oldest first. The wait row says why it waits (RAM, the
/// live-job cap); this says on what — but only as far as the reader may
/// see. One agent works several boards, and a ticket on a board the reader
/// is not on is named to nobody: its ref or title would leak it. Those are a
/// count (`hidden`), never a label.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Behind {
    /// Ticket refs (the title's head when the board does not number tickets)
    /// on boards the reader can see.
    pub visible: Vec<String>,
    /// Live tickets on boards the reader cannot see.
    pub hidden: usize,
}

/// `rows` as (board id, label), split by whether the reader sees the board.
/// Pure.
pub fn split_behind(rows: Vec<(String, String)>, sees: impl Fn(&str) -> bool) -> Behind {
    let mut out = Behind::default();
    for (board, label) in rows {
        if sees(&board) {
            out.visible.push(label);
        } else {
            out.hidden += 1;
        }
    }
    out
}

/// [`Behind`] for `viewer_user_id`, who may see a board exactly when
/// `talaria_boards::board_role` gives them a role on it — the same gate the
/// board and ticket reads answer 403 behind.
pub async fn behind(pg: &PgPool, viewer_user_id: &str, agent_model: &str) -> Behind {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select b.id::text, coalesce( \
                  case when t.ticket_no is not null \
                       then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end, \
                  left(t.title, 40)) \
         from runs r \
         join tasks t on t.id::text = r.subject_id \
         join boards b on b.id = t.board_id \
         where r.kind = 'work-session' and r.state in ('queued', 'running', 'awaiting') \
           and r.input->>'agentModel' = $1 \
         order by r.created_at \
         limit 16",
    )
    .bind(agent_model)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    let mut seen: std::collections::HashMap<String, bool> = Default::default();
    for (board, _) in &rows {
        if !seen.contains_key(board) {
            let ok = matches!(
                talaria_boards::board_role(pg, viewer_user_id, board).await,
                Ok(Some(_))
            );
            seen.insert(board.clone(), ok);
        }
    }
    split_behind(rows, |b| seen.get(b).copied().unwrap_or(false))
}

/// A wait's wire shape with its [`Behind`]: `behind` (the labels the reader
/// may see) and `behindHidden` (how many more there are).
pub fn wire_behind(wire: &mut serde_json::Value, behind: &Behind) {
    wire["behind"] = serde_json::json!(behind.visible);
    wire["behindHidden"] = serde_json::json!(behind.hidden);
}

pub fn phase_of(position: i64) -> String {
    if position <= 1 {
        "next up — waiting for RAM".into()
    } else {
        format!("queued · {} ahead", position - 1)
    }
}

pub fn wire(w: &Wait) -> serde_json::Value {
    serde_json::json!({
        "agentModel": w.agent_model,
        "reason": w.reason,
        "position": w.position,
        "phase": phase_of(w.position),
        "queuedAt": w.queued_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::{Wait, wire};

    #[test]
    fn a_ticket_on_a_board_the_reader_cannot_see_is_a_count_not_a_name() {
        let rows = vec![
            ("eng".to_string(), "PLAT-1".to_string()),
            (
                "private".to_string(),
                "Acquire Foo Corp — term sheet".to_string(),
            ),
            ("eng".to_string(), "PLAT-4".to_string()),
            ("private".to_string(), "DEAL-2".to_string()),
        ];
        let b = super::split_behind(rows, |board| board == "eng");
        assert_eq!(b.visible, vec!["PLAT-1".to_string(), "PLAT-4".to_string()]);
        assert_eq!(b.hidden, 2);
        let mut wire = serde_json::json!({});
        super::wire_behind(&mut wire, &b);
        assert!(!wire.to_string().contains("Foo"), "{wire}");
        assert_eq!(wire["behindHidden"], 2);
    }

    #[test]
    fn first_in_line_is_next_up() {
        let w = Wait {
            agent_model: "doug-engineering".into(),
            reason: "host has 3.0 GiB free".into(),
            queued_ms: 1,
            position: 1,
        };
        let v = wire(&w);
        assert!(v.get("state").is_none());
        assert!(v.get("runId").is_none());
        assert_eq!(v["phase"], "next up — waiting for RAM");
        assert_eq!(v["position"], 1);
        assert_eq!(v["reason"], "host has 3.0 GiB free");
    }

    #[test]
    fn later_tickets_name_how_many_ahead() {
        let w = Wait {
            agent_model: "doug-engineering".into(),
            reason: "host has 3.0 GiB free".into(),
            queued_ms: 1,
            position: 4,
        };
        assert_eq!(wire(&w)["phase"], "queued · 3 ahead");
    }
}

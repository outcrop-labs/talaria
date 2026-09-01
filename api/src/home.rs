// The Home/Today queue pass — port of `homeQueues` from ui/src/server/home.ts.
//
// ONLY THE QUEUES, not `homeSummary`: the digest needs exactly these three
// numbers for every user in the workspace and none of the glance around them.
// Calling the whole summary per user would have run a Docker round trip once
// per recipient, and an email that says "2 tickets in QA" must count them the
// SAME WAY the screen does — so copying the query into the digest was never an
// option in TS and is not one here. One definition, two callers; when the Home
// route family crosses it ports onto this same fn.
//
// The three queues are the whole job description Talaria gives a person:
// triage (inbox — needs a human to assign), review (quality_review — needs
// sign-off), blocked (needs unblocking). Scoped to boards the user can see,
// which is what `board_visibility_sql` encodes and what keeps a digest line
// inside the one visibility model rule 4 of digest.ts states.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;
use crate::boards::board_visibility_sql;

/// One ticket as Home lists it (home.ts `WorkItem`). `updated_ms` is carried
/// because the queue's sort is `updated_at desc` — the column arrives here as
/// the epoch-ms read and is rendered as ISO for shape-parity with the row TS
/// handed back (a Date there, a string on the wire).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub id: String,
    pub board_id: String,
    pub board: String,
    pub ticket_ref: Option<String>,
    pub title: String,
    pub status: String,
    pub updated_at: String,
    /// The bucket SQL decided (`blocked`/`review`/`triage`). TS's interface
    /// doesn't declare it, but its rows go on the wire whole — the field is
    /// part of the payload even though no reader names it.
    pub queue: String,
}

impl WorkItem {
    /// The digest's list line: `TASK-12 · Fix the login loop`, or the bare
    /// title when the board has no numbering.
    pub fn label(&self) -> String {
        match &self.ticket_ref {
            Some(r) if !r.is_empty() => format!("{r} · {}", self.title),
            _ => self.title.clone(),
        }
    }
}

/// The three human queues, each a count of everything in it and a capped list.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HomeQueues {
    pub triage: QueueBucket,
    pub review: QueueBucket,
    pub blocked: QueueBucket,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct QueueBucket {
    /// Everything in the queue — the number the digest's subject states, and
    /// deliberately NOT `items.len()`, which is the same number only until a
    /// queue outgrows the window.
    pub count: usize,
    pub items: Vec<WorkItem>,
}

// Full lists — the Boards console shows everything per queue (capped sanely).
const WINDOW: usize = 100;

/// One row of the pass, `queue` being the bucket SQL already decided. A
/// derived struct over a tuple because the columns (eight after the two
/// timestamps) sit close to sqlx's tuple cap and the names keep the decode
/// honest against a query whose select list is long.
#[derive(Debug, sqlx::FromRow)]
struct QueueRow {
    id: String,
    board_id: String,
    board: String,
    ticket_ref: Option<String>,
    title: String,
    status: String,
    updated_ms: i64,
    queue: String,
}

/// The queue pass on its own: one round trip over the user's visible, active
/// tickets in the three human queues, newest first.
pub async fn home_queues(pg: &PgPool, user_id: &str) -> Result<HomeQueues, sqlx::Error> {
    // AssertSqlSafe: the interpolation is the crate's board-visibility
    // fragment over this query's own bind positions.
    let sql = format!(
        "select t.id::text, t.board_id::text, b.name as board, \
                case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as ticket_ref, \
                t.title, t.status, \
                (trunc(extract(epoch from t.updated_at) * 1000))::bigint as updated_ms, \
                case \
                  when t.status = 'blocked' then 'blocked' \
                  when t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'review') \
                    or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status = 'quality_review') \
                    then 'review' \
                  else 'triage' \
                end as queue \
         from tasks t \
         join boards b on b.id = t.board_id \
         where {} \
           and t.archived_at is null \
           and ( t.status = 'blocked' \
                 or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'review') \
                 or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start) \
                 or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status in ('inbox', 'quality_review')) ) \
         order by t.updated_at desc",
        board_visibility_sql("$1", "$1", false),
    );
    let rows: Vec<QueueRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .fetch_all(pg)
        .await?;

    let item = |r: QueueRow| WorkItem {
        id: r.id,
        board_id: r.board_id,
        board: r.board,
        ticket_ref: r.ticket_ref,
        title: r.title,
        status: r.status,
        updated_at: epoch_ms_to_iso(r.updated_ms),
        queue: r.queue,
    };

    // Three passes over one row set, exactly as TS filters it: the bucket is
    // SQL's decision, count is the whole bucket, and the list is the bucket
    // capped at the window.
    let mut queues = HomeQueues::default();
    for row in rows {
        let (bucket, it) = match row.queue.as_str() {
            "blocked" => (&mut queues.blocked, item(row)),
            "review" => (&mut queues.review, item(row)),
            _ => (&mut queues.triage, item(row)),
        };
        bucket.count += 1;
        if bucket.items.len() < WINDOW {
            bucket.items.push(it);
        }
    }
    Ok(queues)
}

// ── The whole glance (home.ts `homeSummary`) ─────────────────────────────────
//
// The org half: name, an activity pulse everyone sees, and (admins) live
// alerts + today's spend. Failures degrade to quiet, never 500. Fleet health
// is NOT here — it left Home with the Fleet tab; Agents and Observability own
// that question.

/// The org rail's live half (home.ts `OrgGlance`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgGlance {
    /// The business name (Admin → Organization), for the rail's title.
    pub name: String,
    /// A compact recent-activity pulse across the workspace.
    pub activity: Vec<crate::activity::ActivityEvent>,
    /// Admin-only: live alert count (null for members).
    pub alerts: Option<i32>,
    /// Admin-only: today's metered spend (null for members).
    pub cost_today: Option<CostToday>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CostToday {
    pub tokens: i32,
    pub usd: serde_json::Number,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HomeSummary {
    pub org: OrgGlance,
    pub queues: HomeQueues,
    pub unread: i32,
    pub boards: i32,
}

/// `homeSummary` — the Home/Today landing in one round-shape. `role` is the
/// caller's (`user.role`), and only admins pay for the alerts count and the
/// cost overview.
pub async fn home_summary(
    state: &crate::state::AppState,
    user_id: &str,
    is_admin: bool,
) -> Result<HomeSummary, sqlx::Error> {
    let pg = &state.pg;
    let queues = home_queues(pg, user_id).await?;

    let vis = board_visibility_sql("$1", "$1", false);
    let boards_sql = format!(
        "select count(distinct b.id)::int as boards from boards b where {vis}"
    );
    let (unread_res, boards_res) = tokio::join!(
        sqlx::query_as::<_, (i32,)>(
            "select count(*)::int from notifications where user_id = $1::uuid and read_at is null"
        )
        .bind(user_id)
        .fetch_one(pg),
        sqlx::query_as::<_, (i32,)>(sqlx::AssertSqlSafe(boards_sql.as_str()))
            .bind(user_id)
            .fetch_one(pg)
    );
    let ((unread,), (boards,)) = (unread_res?, boards_res?);

    // The glance's four reads all fold their own failures, exactly as TS's
    // `.catch`es do: orgProfile to quiet strings, the feed to empty, alerts
    // and cost to null.
    let profile_fut = crate::org::org_profile(pg);
    let activity_fut = async {
        crate::activity::activity_feed(pg, user_id, &[], 8, is_admin)
            .await
            .unwrap_or_default()
    };
    let alert_count_fut = async {
        if is_admin {
            Some(crate::alerts::compute_alerts(state, user_id).await.len() as i32)
        } else {
            None
        }
    };
    let cost_fut = async {
        if is_admin {
            crate::gateway::usage::cost_overview(pg).await.ok()
        } else {
            None
        }
    };
    let (profile, activity, alert_count, cost) =
        tokio::join!(profile_fut, activity_fut, alert_count_fut, cost_fut);

    Ok(HomeSummary {
        org: OrgGlance {
            name: profile.name,
            activity,
            alerts: alert_count,
            cost_today: cost.map(|c| CostToday {
                tokens: c.totals.today.prompt + c.totals.today.completion,
                usd: c.totals.today.cost,
            }),
        },
        queues,
        unread,
        boards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, board: &str) -> WorkItem {
        WorkItem {
            id: id.into(),
            board_id: "b1".into(),
            board: board.into(),
            ticket_ref: Some(format!("TASK-{id}")),
            title: format!("ticket {id}"),
            status: "inbox".into(),
            updated_at: "2026-08-29T00:00:00.000Z".into(),
            queue: "triage".into(),
        }
    }

    #[test]
    fn the_label_joins_ref_and_title_or_stands_alone() {
        assert_eq!(ticket("12", "Ops").label(), "TASK-12 · ticket 12");
        let bare = WorkItem {
            ticket_ref: None,
            ..ticket("1", "Ops")
        };
        assert_eq!(bare.label(), "ticket 1");
    }
}

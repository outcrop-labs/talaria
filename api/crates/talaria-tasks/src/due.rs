// Due-date reminders: the `task-due` job (docs/notifications/producers.md
// "Due dates"). Every 15 minutes it finds the open tickets that crossed a
// threshold since the last pass and tells the people who asked to hear it:
//
//   due soon   the due date is less than a day ahead (SOON_HOURS)
//   overdue    the due date has passed, less than a day ago (CATCH_UP_HOURS)
//
// ONCE PER DUE DATE. `tasks.due_soon_for` and `tasks.overdue_for` hold the due
// date each threshold last fired for. A pass marks a ticket after its fan-out,
// compare-and-set on the due date it read, so moving the due date re-arms both
// (the marker no longer equals it), and a ticket that is already past due at
// its first pass gets "overdue" alone. A crash between the fan-out and the mark
// files the reminder again on the next pass, into the same open row
// (`group_key::task_due`, Realert): it re-alerts rather than doubling.
//
// NO FLOOD. "Overdue" fires only in the day after the due date. A ticket that
// was already long overdue when someone opted in, or when this shipped, is on
// the board for everyone to see and is not news. A pass takes at most
// CLAIM_LIMIT tickets, soonest due first; the rest wait for the next pass.
//
// OPT-IN, DECIDED AT THE MOMENT. Who hears it is each person's setting for the
// board (talaria_notify::boards, event `due`). No preset but All names it, so
// with nobody's setting stored nobody is told, and existing users see no
// change. The ticket's own audience (watchers, assigned people, the creator —
// `ticket_audience`, access re-checked now) gets a `task-due` row; the people
// who hear the whole board get it on the board's activity row. A watch turns
// on nothing new. A ticket that crosses a threshold while nobody on its board
// could hear one is marked all the same, with no audience read
// (`boards::due_possible`): a setting changed later applies to the next
// threshold, never to one already passed.
//
// SKIPPED: archived tickets, tickets on archived boards, and closed tickets —
// a done column on the ticket's own board, or failed / cancelled. That is the
// same "open" question talaria-channel-plan asks, through the statuses
// module's own fragment. A closed ticket is not marked, so one reopened while
// its due date is still in the band is still reminded.
//
// The reminder is the clock's, not a write's: a ticket a tracker sync keeps
// (talaria-board-sync, whose writes are quiet) reminds like any other.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use sqlx::PgPool;
use talaria_notify::boards::{self as board_prefs, BoardEvent};
use talaria_notify::{NotifyDeps, group_key};
use talaria_scheduler::{JobName, JobSpec};
use talaria_statuses::OFF_BOARD_STATUSES;
use talaria_tasks_types::json_strings;

use super::{TaskNotification, file_decided, ticket_audience, try_bystanders};

/// Who a reminder says caused it. `task_actor` answers a system actor
/// ("Due dates") for this id: never a person, so nobody is left out as the
/// actor.
pub const ACTOR: &str = "task-due";

/// The words a reminder row's actor carries.
pub const ACTOR_LABEL: &str = "Due dates";

/// "Due soon": the due date is at most this far ahead.
pub const SOON_HOURS: i32 = 24;
/// "Overdue" fires up to this long after the due date, and never later.
pub const CATCH_UP_HOURS: i32 = 24;

const HOUR_MS: i64 = 3_600_000;
const EVERY_MS: u64 = 15 * 60_000;
const FIRST_DELAY_MS: u64 = 2 * 60_000;
const MAX_RUN_MS: u64 = 5 * 60_000;
const CLAIM_LIMIT: i64 = 200;

/// The two thresholds a due date has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threshold {
    /// Less than a day ahead.
    Soon,
    /// Passed, less than a day ago.
    Overdue,
}

impl Threshold {
    /// What a reminder's title says after the ticket's ref.
    pub fn words(self) -> &'static str {
        match self {
            Threshold::Soon => "due soon",
            Threshold::Overdue => "overdue",
        }
    }
}

/// The threshold a ticket owes a reminder for at `now_ms`, if any. Past due
/// (within CATCH_UP_HOURS) and "overdue" not yet fired for this due date:
/// overdue. Ahead (within SOON_HOURS) and "due soon" not yet fired for it:
/// due soon. Pure; the claim's SQL is the same predicate, and the live test
/// holds the two together.
pub fn owed(
    due_ms: i64,
    now_ms: i64,
    soon_for_ms: Option<i64>,
    overdue_for_ms: Option<i64>,
) -> Option<Threshold> {
    if due_ms <= now_ms {
        let in_band = due_ms > now_ms - i64::from(CATCH_UP_HOURS) * HOUR_MS;
        (in_band && overdue_for_ms != Some(due_ms)).then_some(Threshold::Overdue)
    } else {
        let in_band = due_ms <= now_ms + i64::from(SOON_HOURS) * HOUR_MS;
        (in_band && soon_for_ms != Some(due_ms)).then_some(Threshold::Soon)
    }
}

/// One ticket the scan read, with the database's own clock.
#[derive(Debug, sqlx::FromRow)]
struct DueRow {
    id: String,
    board_id: String,
    title: String,
    ticket_ref: Option<String>,
    assignees: serde_json::Value,
    created_by: String,
    due_ms: i64,
    soon_for_ms: Option<i64>,
    overdue_for_ms: Option<i64>,
    now_ms: i64,
}

/// The open tickets in the band that owe a reminder, soonest due first.
async fn claim(pg: &PgPool) -> Result<Vec<DueRow>, sqlx::Error> {
    let off_board = OFF_BOARD_STATUSES
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "select t.id::text as id, t.board_id::text as board_id, t.title, \
           case when t.ticket_no is not null \
                then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as ticket_ref, \
           t.assignees, t.created_by, \
           (trunc(extract(epoch from t.due_date) * 1000))::bigint as due_ms, \
           (trunc(extract(epoch from t.due_soon_for) * 1000))::bigint as soon_for_ms, \
           (trunc(extract(epoch from t.overdue_for) * 1000))::bigint as overdue_for_ms, \
           (trunc(extract(epoch from now()) * 1000))::bigint as now_ms \
         from tasks t join boards b on b.id = t.board_id \
         where t.due_date is not null and t.archived_at is null and b.archived_at is null \
           and t.due_date > now() - $1::int * interval '1 hour' \
           and t.due_date <= now() + $2::int * interval '1 hour' \
           and ((t.due_date > now() and t.due_soon_for is distinct from t.due_date) \
             or (t.due_date <= now() and t.overdue_for is distinct from t.due_date)) \
           and t.status not in ({off_board}) \
           and not {done} \
         order by t.due_date asc limit $3",
        done = talaria_statuses::status_category_sql("done", &["done"]),
    );
    // AssertSqlSafe: both interpolations are the statuses module's own
    // constants — its category fragment and its off-board keys. No user input
    // reaches the string; the bands and the limit are binds.
    sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(CATCH_UP_HOURS)
        .bind(SOON_HOURS)
        .bind(CLAIM_LIMIT)
        .fetch_all(pg)
        .await
}

/// Record that `th` fired for the due date the scan read — only while the
/// ticket still has it: a due date moved meanwhile is a new one, owed its own
/// reminders. The marker is the column's own value, never a rounded copy.
async fn mark(pg: &PgPool, task_id: &str, due_ms: i64, th: Threshold) -> Result<(), sqlx::Error> {
    let sql = match th {
        Threshold::Soon => {
            "update tasks set due_soon_for = due_date \
             where id = $1::uuid and (trunc(extract(epoch from due_date) * 1000))::bigint = $2"
        }
        Threshold::Overdue => {
            "update tasks set overdue_for = due_date \
             where id = $1::uuid and (trunc(extract(epoch from due_date) * 1000))::bigint = $2"
        }
    };
    sqlx::query(sql)
        .bind(task_id)
        .bind(due_ms)
        .execute(pg)
        .await?;
    Ok(())
}

/// Tell one ticket's reminder to everyone whose setting for its board names
/// due dates: the ticket's own audience a `task-due` row, the people who hear
/// the whole board its activity row. Every read behind it — the ticket's
/// audience, the board's membership, the settings — answers its error rather
/// than a fallback: the built-in other events fall back to names no due
/// date, so for a reminder a fallback would be silence, with the ticket then
/// marked. An error leaves the ticket unmarked and the next pass tries again.
async fn remind(notify: &NotifyDeps, t: &DueRow, th: Threshold) -> Result<(), sqlx::Error> {
    let assignees = json_strings(&t.assignees);
    let mut candidates =
        ticket_audience(&notify.pg, &t.id, &t.board_id, &assignees, &t.created_by).await?;
    let rest = try_bystanders(&notify.pg, &t.board_id, &candidates, &HashSet::new()).await?;
    candidates.extend(rest);
    let decided =
        board_prefs::try_listeners(&notify.pg, &t.board_id, BoardEvent::Due, &candidates).await?;
    let label = t.ticket_ref.clone().unwrap_or_else(|| t.title.clone());
    let n = TaskNotification {
        kind: "task-due".into(),
        title: format!("{label}: {}", th.words()),
        body: Some(t.title.clone()),
        href: Some(format!("/boards/{}/{}", t.board_id, t.id)),
        task_id: t.id.clone(),
        status_burst: None,
        group_key: Some(group_key::task_due(&t.id)),
    };
    let line = n.title.clone();
    file_decided(
        notify,
        &t.board_id,
        BoardEvent::Due,
        decided,
        ACTOR,
        Some(&n),
        &line,
    )
    .await;
    Ok(())
}

/// One pass: every ticket owed a reminder, told and marked. Answers a line
/// for the scheduler's log, or None when nothing was owed.
pub async fn tick(notify: &NotifyDeps) -> Result<Option<String>, String> {
    let rows = claim(&notify.pg).await.map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok(None);
    }
    // One probe per board per pass: most boards have nobody listening.
    let mut listening: HashMap<String, bool> = HashMap::new();
    let (mut told, mut unheard, mut failed) = (0usize, 0usize, 0usize);
    for t in &rows {
        let Some(th) = owed(t.due_ms, t.now_ms, t.soon_for_ms, t.overdue_for_ms) else {
            continue;
        };
        let hears = match listening.get(&t.board_id) {
            Some(v) => *v,
            None => {
                let v = board_prefs::due_possible(&notify.pg, &t.board_id).await;
                listening.insert(t.board_id.clone(), v);
                v
            }
        };
        if hears {
            if let Err(e) = remind(notify, t, th).await {
                tracing::error!("[task-due] reminder for {} not sent: {e}", t.id);
                failed += 1;
                continue;
            }
            told += 1;
        } else {
            unheard += 1;
        }
        if let Err(e) = mark(&notify.pg, &t.id, t.due_ms, th).await {
            tracing::error!("[task-due] marking {} failed: {e}", t.id);
        }
    }
    if told + unheard + failed == 0 {
        return Ok(None);
    }
    let line =
        format!("{told} ticket(s) reminded, {unheard} with nobody listening, {failed} to retry");
    if failed > 0 {
        return Err(line);
    }
    Ok(Some(line))
}

/// TEST ONLY: `TALARIA_TEST_TASK_DUE_EVERY_MS=<n>` runs the scan every n ms
/// (first run after n ms), so a live run can watch it. Read once; loud at
/// registration.
fn test_every_ms() -> Option<u64> {
    static V: LazyLock<Option<u64>> = LazyLock::new(|| {
        std::env::var("TALARIA_TEST_TASK_DUE_EVERY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v >= 1_000)
    });
    *V
}

/// The job: every 15 minutes, leased (two instances never scan at once).
pub fn job_spec(notify: NotifyDeps) -> JobSpec {
    let test = test_every_ms();
    JobSpec {
        name: JobName::TaskDue,
        every_ms: test.unwrap_or(EVERY_MS),
        first_run_delay_ms: Some(test.unwrap_or(FIRST_DELAY_MS)),
        max_run_ms: Some(MAX_RUN_MS),
        per_instance: false,
        run: Arc::new(move || {
            let notify = notify.clone();
            Box::pin(async move { tick(&notify).await })
        }),
    }
}

/// Register the job (talaria-jobs' boot list).
pub fn register_task_due_job(notify: NotifyDeps) {
    if let Some(ms) = test_every_ms() {
        tracing::warn!(
            "[task-due] TALARIA_TEST_TASK_DUE_EVERY_MS={ms}: the due-date scan runs every {ms} ms. TEST ONLY."
        );
    }
    talaria_scheduler::register_job(job_spec(notify));
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const H: i64 = HOUR_MS;

    #[test]
    fn a_due_date_arms_each_threshold_once() {
        // More than a day ahead: nothing yet.
        assert_eq!(owed(NOW + 25 * H, NOW, None, None), None);
        // A day ahead, to the millisecond: due soon.
        assert_eq!(owed(NOW + 24 * H, NOW, None, None), Some(Threshold::Soon));
        assert_eq!(owed(NOW + H, NOW, None, None), Some(Threshold::Soon));
        // Fired for this due date: not again.
        assert_eq!(owed(NOW + H, NOW, Some(NOW + H), None), None);
        // Past due: overdue, whether or not "due soon" fired.
        assert_eq!(
            owed(NOW - H, NOW, Some(NOW - H), None),
            Some(Threshold::Overdue)
        );
        assert_eq!(owed(NOW, NOW, None, None), Some(Threshold::Overdue));
        // A ticket already past due at its first pass gets "overdue" alone:
        // "due soon" is owed only while the due date is ahead.
        assert_eq!(
            owed(NOW - 60_000, NOW, None, None),
            Some(Threshold::Overdue)
        );
        // Fired for this due date: not again.
        assert_eq!(owed(NOW - H, NOW, Some(NOW - H), Some(NOW - H)), None);
    }

    #[test]
    fn moving_the_due_date_re_arms_both() {
        let old = NOW - 2 * H;
        // Both fired for the old date; the date moves a few hours out.
        assert_eq!(
            owed(NOW + 3 * H, NOW, Some(old), Some(old)),
            Some(Threshold::Soon)
        );
        // …and once it passes, "overdue" fires for the new date too.
        assert_eq!(
            owed(NOW + 3 * H, NOW + 4 * H, Some(NOW + 3 * H), Some(old)),
            Some(Threshold::Overdue)
        );
    }

    #[test]
    fn a_long_overdue_ticket_is_not_news() {
        assert_eq!(owed(NOW - 24 * H, NOW, None, None), None);
        assert_eq!(owed(NOW - 30 * 24 * H, NOW, None, None), None);
        // Just inside the day after: still owed.
        assert_eq!(
            owed(NOW - 24 * H + 1, NOW, None, None),
            Some(Threshold::Overdue)
        );
    }

    #[test]
    fn the_words_are_pinned() {
        assert_eq!(Threshold::Soon.words(), "due soon");
        assert_eq!(Threshold::Overdue.words(), "overdue");
        assert_eq!(ACTOR, "task-due");
        assert_eq!((SOON_HOURS, CATCH_UP_HOURS), (24, 24));
    }

    #[tokio::test]
    async fn the_job_is_leased_every_fifteen_minutes() {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://x@localhost/x")
            .unwrap();
        let spec = job_spec(NotifyDeps::publishing(pg, None));
        assert_eq!(spec.name, JobName::TaskDue);
        assert_eq!(spec.name.as_str(), "task-due");
        if test_every_ms().is_none() {
            assert_eq!(spec.every_ms, 15 * 60_000);
        }
        assert!(!spec.per_instance, "the scan is leased");
    }
}

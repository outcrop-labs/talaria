// Tasks — the port of ui/src/server/tasks.ts. One ticket is a card on a
// board with a ref, effort, a structured result, comments, activity,
// watchers, dependencies and a quality-review approval gate; this file is
// the whole engine under the routes — every read, every write, the
// activity cascade, the notification fan-out and both sides of the agent
// authority question.
//
// WHAT ALREADY LIVED HERE: the pieces earlier slices needed (the mixed
// assignee helpers realtime and approvals read, `task_board_id`, and the
// agent-write POLICY plane — the one predicate every gate, session and
// heartbeat asks of a ticket). This slice grows the file to the whole
// module: the CRUD, assignment and quality planes.
//
// TS's `BoardFacts` memo (board info + agent policy + status meta, one pass
// per loop) is a per-pass CACHE, never an answer; the Rust port reads
// through, and the boards it resolves twice per call are the queries the
// cache existed to save. The same divergence is documented on
// `agent_ticket_refusal`.

use crate::agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model};
use crate::agent_writes::{WriteAuthor, guard_agent_fields, guard_agent_write};
use crate::boards::{board_allows_agent, board_info, board_role};
use crate::error::house_error;
use crate::gateway::usage::task_usage;
use crate::judge::list_judge_reviews;
use crate::labels::ensure_labels;
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::realtime::{BoardEvent, RealtimeDeps, publish_board};
use crate::runs::run::RunDeps;
use crate::statuses::{OFF_BOARD_STATUSES, StatusMeta, status_meta};
use crate::work_dispatch::{DispatchTicket, coexistence_dispatch_deps, maybe_dispatch_ticket};
use axum::http::StatusCode;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ── The edges a ticket write touches beyond its own row ─────────────────────

/// Everything tasks.ts reaches for past its own SQL: the board's realtime
/// topic, the notification fan-out, and the work-dispatch push. Clone-cheap
/// (all Arcs and pools), so the fire-and-forget legs TS starts with `void …`
/// can take a copy into their own task.
///
/// `dispatch` is Option because assembling the runs edge needs a live Redis
/// connection and a route may be serving while Redis is down. None is TS's
/// `void maybeDispatchTicket(…).catch(() => {})` posture: the ticket write
/// survives, the push stands down.
#[derive(Clone)]
pub struct TaskDeps {
    pub pg: PgPool,
    pub realtime: RealtimeDeps,
    pub notify: NotifyDeps,
    pub dispatch: Option<RunDeps>,
}

impl TaskDeps {
    /// From the pieces a route handler holds. Realtime unreachable publishes
    /// nothing, notification rows still land, dispatch stands down — each is
    /// the TS failure posture for that edge.
    pub fn coexistence(pg: PgPool, redis: Option<redis::aio::ConnectionManager>) -> Self {
        let realtime = RealtimeDeps::publish_only(redis.clone());
        let notify = NotifyDeps::publishing(pg.clone(), redis.clone());
        let dispatch =
            redis.map(|conn| coexistence_dispatch_deps(pg.clone(), conn, realtime.clone()));
        TaskDeps {
            pg,
            realtime,
            notify,
            dispatch,
        }
    }
}

/// tasks.ts's two throw shapes, kept apart because the route answers them
/// differently: a plain `Error` is a refused write (400 — the request was
/// well-formed but the board says no), `HumanApprovalRequired` is 403 (the
/// write needs a person), and a database failure is 500.
pub enum TaskError {
    /// `throw new Error(sentence)` — the sentence is the product's refusal.
    Refusal(String),
    /// HumanApprovalRequired — a write that needs a person.
    ApprovalRequired(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for TaskError {
    fn from(e: sqlx::Error) -> Self {
        TaskError::Db(e)
    }
}

impl TaskError {
    pub fn message(&self) -> String {
        match self {
            TaskError::Refusal(m) | TaskError::ApprovalRequired(m) => m.clone(),
            TaskError::Db(e) => e.to_string(),
        }
    }

    /// The status the route answers with — TS's `instanceof` branch, one
    /// place, so a new error kind cannot inherit the wrong one.
    pub fn status(&self) -> StatusCode {
        match self {
            TaskError::Refusal(_) => StatusCode::BAD_REQUEST,
            TaskError::ApprovalRequired(_) => StatusCode::FORBIDDEN,
            TaskError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The house envelope, ready to return.
    pub fn into_response(self) -> axum::response::Response {
        house_error(self.status(), &self.message())
    }
}

pub type TaskResult<T> = Result<T, TaskError>;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Mixed assignees ──────────────────────────────────────────────────────────
// The assignees array mixes AGENT model ids (bare strings, unchanged — the
// heartbeat/outreach `@>` predicates keep matching) and HUMANS as
// `user:<uuid>`. Helpers below split the two worlds.

/// tasks.ts isHumanAssignee.
pub fn is_human_assignee(a: &str) -> bool {
    a.starts_with("user:")
}

/// tasks.ts humanAssigneeIds: the human half of a mixed assignees array, with
/// the `user:` prefix stripped.
pub fn human_assignee_ids(assignees: &[String]) -> Vec<String> {
    assignees
        .iter()
        .filter(|a| is_human_assignee(a))
        .map(|a| a[5..].to_string())
        .collect()
}

/// tasks.ts agentAssignees: the agent half of a mixed assignees array, bare
/// model ids unchanged — the strings the heartbeat's `@>` predicates and the
/// dispatch walk match.
pub fn agent_assignees(assignees: &[String]) -> Vec<String> {
    assignees
        .iter()
        .filter(|a| !is_human_assignee(a))
        .cloned()
        .collect()
}

// ── The row and its select ───────────────────────────────────────────────────

/// The ticket as every reader serves it (TS `Task`): TASK_SELECT's column
/// list in order, camelCase on the wire. Timestamps are ISO strings —
/// postgres.js parses timestamptz into Dates that JSON-serialize as
/// toISOString(), so the Rust select fetches epoch-ms and shapes them here,
/// the same contract boards.rs established.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub board_id: String,
    /// `PREFIX-12` — the board's prefix and the per-board counter, or None
    /// for pre-ref tickets.
    pub ticket_ref: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub effort: Option<String>,
    pub assignees: Vec<String>,
    pub created_by: String,
    pub due_date: Option<String>,
    pub start_date: Option<String>,
    pub color: Option<String>,
    pub tags: Vec<String>,
    pub attachments: serde_json::Value,
    pub time_spent_seconds: i64,
    /// Human planning estimate; the `::float8` cast in the select is also
    /// what makes the estimate-activity comparison numeric on this side —
    /// postgres.js hands `numeric` back as a STRING, which is the entire
    /// reason TS needs `pgNum` there and this port does not.
    pub estimated_hours: Option<f64>,
    pub parent_id: Option<String>,
    pub comment_count: i32,
    pub outcome: Option<String>,
    pub resolution: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub archived_at: Option<String>,
}

/// TASK_SELECT, verbatim but for the sqlx needs: uuid columns `::text`-cast,
/// timestamptz as epoch-ms (shaped to ISO by the row mapping), aliases
/// matching the FromRow struct below. The trailing suffix (where/order) is
/// the caller's, composed with format! under AssertSqlSafe.
const TASK_SELECT: &str = "select t.id::text as id, t.board_id::text as board_id, \
  case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as ticket_ref, \
  t.title, t.description, t.status, t.priority, t.effort, t.assignees, t.created_by as created_by, \
  (trunc(extract(epoch from t.due_date) * 1000))::bigint as due_date_ms, \
  (trunc(extract(epoch from t.start_date) * 1000))::bigint as start_date_ms, \
  t.color, t.tags, t.attachments, t.time_spent_seconds as time_spent_seconds, \
  t.estimated_hours::float8 as estimated_hours, t.parent_id::text as parent_id, \
  (select count(*)::int from task_comments c where c.task_id = t.id) as comment_count, \
  t.outcome, t.resolution, t.error_message as error_message, \
  (trunc(extract(epoch from t.created_at) * 1000))::bigint as created_at_ms, \
  (trunc(extract(epoch from t.updated_at) * 1000))::bigint as updated_at_ms, \
  (trunc(extract(epoch from t.completed_at) * 1000))::bigint as completed_at_ms, \
  (trunc(extract(epoch from t.archived_at) * 1000))::bigint as archived_at_ms \
  from tasks t join boards b on b.id = t.board_id";

/// The wide row exactly as selected. jsonb lands as `Value` and is typed
/// (`Vec<String>` for the string arrays) in the `Task` mapping — a malformed
/// array reads as empty rather than failing the whole ticket, which is also
/// the only shape a not-null-default-'[]' column can be in.
#[derive(sqlx::FromRow)]
struct TaskRow {
    id: String,
    board_id: String,
    ticket_ref: Option<String>,
    title: String,
    description: Option<String>,
    status: String,
    priority: String,
    effort: Option<String>,
    assignees: serde_json::Value,
    created_by: String,
    due_date_ms: Option<i64>,
    start_date_ms: Option<i64>,
    color: Option<String>,
    tags: serde_json::Value,
    attachments: serde_json::Value,
    time_spent_seconds: i64,
    estimated_hours: Option<f64>,
    parent_id: Option<String>,
    comment_count: i32,
    outcome: Option<String>,
    resolution: Option<String>,
    error_message: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    completed_at_ms: Option<i64>,
    archived_at_ms: Option<i64>,
}

fn json_strings(v: &serde_json::Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

impl From<TaskRow> for Task {
    fn from(r: TaskRow) -> Self {
        Task {
            id: r.id,
            board_id: r.board_id,
            ticket_ref: r.ticket_ref,
            title: r.title,
            description: r.description,
            status: r.status,
            priority: r.priority,
            effort: r.effort,
            assignees: json_strings(&r.assignees),
            created_by: r.created_by,
            due_date: r.due_date_ms.map(epoch_ms_to_iso),
            start_date: r.start_date_ms.map(epoch_ms_to_iso),
            color: r.color,
            tags: json_strings(&r.tags),
            attachments: r.attachments,
            time_spent_seconds: r.time_spent_seconds,
            estimated_hours: r.estimated_hours,
            parent_id: r.parent_id,
            comment_count: r.comment_count,
            outcome: r.outcome,
            resolution: r.resolution,
            error_message: r.error_message,
            created_at: epoch_ms_to_iso(r.created_at_ms),
            updated_at: epoch_ms_to_iso(r.updated_at_ms),
            completed_at: r.completed_at_ms.map(epoch_ms_to_iso),
            archived_at: r.archived_at_ms.map(epoch_ms_to_iso),
        }
    }
}

/// The dispatch walk's view of a ticket, from the shape this module already
/// holds (TS passes the same task object through).
pub fn dispatch_ticket_of(task: &Task) -> DispatchTicket {
    DispatchTicket {
        id: task.id.clone(),
        board_id: task.board_id.clone(),
        status: task.status.clone(),
        assignees: task.assignees.clone(),
        archived_at: task.archived_at.clone(),
    }
}

/// Minimal shape for dependency links (blocked-by / blocks) — LINK_SELECT.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLink {
    pub id: String,
    pub ticket_ref: Option<String>,
    pub title: String,
    pub status: String,
}

const LINK_SELECT: &str = "select t.id::text as id, \
  case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as ticket_ref, \
  t.title, t.status from tasks t join boards b on b.id = t.board_id";

/// tasks.ts getTask().boardId, and nothing else about the row — the watch
/// gate and every publish-after-write call select exactly the one column
/// they need.
pub async fn task_board_id(pg: &PgPool, task_id: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(task_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(board_id,)| board_id))
}

/// tasks.ts listBoardTasks: a board's tickets, newest-update first; archived
/// tickets only when the caller asks for them.
pub async fn list_board_tasks(
    pg: &PgPool,
    board_id: &str,
    include_archived: bool,
) -> Result<Vec<Task>, sqlx::Error> {
    let where_clause = if include_archived {
        "t.board_id = $1::uuid"
    } else {
        "t.board_id = $1::uuid and t.archived_at is null"
    };
    // AssertSqlSafe: the interpolation is this module's TASK_SELECT/where
    // composition — the board id stays a bind, as TS's sql.unsafe($1) does.
    let sql = format!("{TASK_SELECT} where {where_clause} order by t.updated_at desc");
    let rows: Vec<TaskRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(board_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(Task::from).collect())
}

/// tasks.ts getTask — the one-ticket read every write re-reads through.
pub async fn get_task(pg: &PgPool, id: &str) -> Result<Option<Task>, sqlx::Error> {
    let sql = format!("{TASK_SELECT} where t.id = $1::uuid");
    let row: Option<TaskRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(Task::from))
}

/// tasks.ts getTaskFull: a ticket and everything hanging off it, read
/// concurrently. `TaskUsage`/`JudgeReview` cross in their own modules; this
/// is the only place they ride together.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFull {
    pub task: Task,
    pub comments: Vec<TaskComment>,
    pub activity: Vec<TaskActivity>,
    pub watchers: Vec<String>,
    pub reviews: Vec<QualityReview>,
    pub judge_reviews: Vec<crate::judge::JudgeReview>,
    pub blocked_by: Vec<TaskLink>,
    pub blocks: Vec<TaskLink>,
    pub usage: crate::gateway::usage::TaskUsage,
}

pub async fn get_task_full(pg: &PgPool, id: &str) -> Result<Option<TaskFull>, sqlx::Error> {
    let Some(task) = get_task(pg, id).await? else {
        return Ok(None);
    };
    let ((blocked_by, blocks), comments, activity, watchers, reviews, judge_reviews, usage) = tokio::try_join!(
        list_dependencies(pg, id),
        list_comments(pg, id),
        list_activity(pg, id),
        list_watchers(pg, id),
        list_reviews(pg, id),
        list_judge_reviews(pg, id),
        task_usage(pg, id),
    )?;
    Ok(Some(TaskFull {
        task,
        comments,
        activity,
        watchers,
        reviews,
        judge_reviews,
        blocked_by,
        blocks,
        usage,
    }))
}

// ── Notifications: who hears about a ticket ──────────────────────────────────

/// The notification payload a ticket fan-out files (TS's inline object —
/// kind, title, body, href).
struct TaskNotification {
    kind: String,
    title: String,
    body: Option<String>,
    href: Option<String>,
}

/// tasks.ts notifyTaskUsers: file a notification for each user, deduped,
/// never the actor. `addNotification`'s per-user `.catch(() => {})` keeps one
/// person's failed row from costing the rest theirs — logged here, never
/// propagated.
async fn notify_task_users(
    notify: &NotifyDeps,
    user_ids: &[String],
    actor: &str,
    n: &TaskNotification,
) {
    if user_ids.is_empty() {
        return;
    }
    // Self-exclusion by actor EMAIL: the actor id is an email for humans, a
    // model id for agents and `judge:<model>` for the platform, and only the
    // email names a row in users.
    let self_id: Option<String> = if actor.contains('@') {
        sqlx::query_as::<_, (String,)>("select id::text from users where lower(email) = $1 limit 1")
            .bind(actor.to_lowercase())
            .fetch_optional(&notify.pg)
            .await
            .ok()
            .flatten()
            .map(|(id,)| id)
    } else {
        None
    };
    let mut seen: HashSet<String> = HashSet::new();
    for user_id in user_ids {
        if !seen.insert(user_id.clone()) {
            continue;
        }
        if Some(user_id.as_str()) == self_id.as_deref() {
            continue;
        }
        let input = NotificationInput {
            kind: &n.kind,
            title: &n.title,
            body: n.body.as_deref(),
            href: n.href.as_deref(),
        };
        if let Err(e) = add_notification(notify, user_id, &input).await {
            tracing::error!("tasks: notification for {user_id} failed: {e}");
        }
    }
}

/// tasks.ts ticketAudience: everyone who may be TOLD about an event on this
/// ticket — the people watching it and the humans assigned to it, each
/// confirmed against the board AS IT STANDS NOW.
///
/// ASKED AT FAN-OUT TIME, NOT TRUSTED FROM THE WRITE. Both halves are
/// validated when they are written (`invalid_assignee` on the ticket write
/// routes, `add_watcher` below) and neither check survives the day after:
/// unsharing a board deletes the membership row and touches nothing else, so
/// the stored assignee/watcher strings keep naming people who now get a 403
/// from the link they would be mailed. `maybe_dispatch_ticket` already works
/// this way for the AGENT audience (it re-asks `agent_ticket_refusal`); this
/// is the same rule for the human one.
async fn ticket_audience(
    pg: &PgPool,
    task_id: &str,
    board_id: &str,
    assignees: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    let mut audience: Vec<String> = resolve_watchers(pg, task_id, Some(board_id))
        .await?
        .into_iter()
        .map(|w| w.user_id)
        .collect();
    for user_id in human_assignee_ids(assignees) {
        if board_role(pg, &user_id, board_id).await?.is_some() {
            audience.push(user_id);
        }
    }
    Ok(audience)
}

// ── Sub-task structure ───────────────────────────────────────────────────────

/// tasks.ts assertValidParent: one level deep, same board, no self-cycle.
/// Every sentence is the product's refusal, not a validation error.
pub async fn assert_valid_parent(
    pg: &PgPool,
    task_id: Option<&str>,
    parent_id: &str,
    board_id: &str,
) -> TaskResult<()> {
    if let Some(t) = task_id
        && t == parent_id
    {
        return Err(TaskError::Refusal(
            "a ticket cannot be its own parent".into(),
        ));
    }
    let parent = get_task(pg, parent_id).await?;
    let bad_board = match &parent {
        None => true,
        Some(p) => p.board_id != board_id,
    };
    if bad_board {
        return Err(TaskError::Refusal(
            "parent must be a ticket on the same board".into(),
        ));
    }
    if parent.as_ref().is_some_and(|p| p.parent_id.is_some()) {
        return Err(TaskError::Refusal(
            "sub-tasks go one level deep — that ticket is already a sub-task".into(),
        ));
    }
    if let Some(t) = task_id {
        let kids: Option<(i32,)> =
            sqlx::query_as("select 1 from tasks where parent_id = $1::uuid limit 1")
                .bind(t)
                .fetch_optional(pg)
                .await?;
        if kids.is_some() {
            return Err(TaskError::Refusal(
                "this ticket has sub-tasks of its own — it cannot become a sub-task".into(),
            ));
        }
    }
    Ok(())
}

// ── Create ───────────────────────────────────────────────────────────────────

/// tasks.ts createTask's input — the route's validated body, minimally
/// typed. Optional strings are ISO timestamps; `estimated_hours` is numeric.
pub struct NewTask<'a> {
    pub board_id: &'a str,
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub assignees: &'a [String],
    pub due_date: Option<&'a str>,
    pub start_date: Option<&'a str>,
    pub color: Option<&'a str>,
    pub estimated_hours: Option<f64>,
    pub parent_id: Option<&'a str>,
    pub tags: &'a [String],
    pub created_by: &'a str,
}

pub async fn create_task(deps: &TaskDeps, input: &NewTask<'_>) -> TaskResult<Task> {
    let pg = &deps.pg;
    let assignees: Vec<String> = input.assignees.to_vec();
    let meta = status_meta(pg, input.board_id).await?;
    // A ticket is BORN in intake, or in the pickup queue when it arrives
    // already assigned. Neither key is invented: `assigned_key` is None on a
    // board with no usable agent-start column, and the honest landing spot
    // then is intake — assigned to someone, not yet in anyone's pickup
    // queue, and therefore not dispatched. `default_key` None means the
    // board has no intake column at all, which no legal board can reach;
    // say so instead of dropping the ticket into whatever column happened
    // to sort first (that fallback could resolve to the system Blocked
    // column).
    let status = match if !assignees.is_empty() {
        meta.assigned_key
            .clone()
            .or_else(|| meta.default_key.clone())
    } else {
        meta.default_key.clone()
    } {
        Some(s) => s,
        None => {
            let board = board_label(pg, input.board_id).await?;
            return Err(TaskError::Refusal(format!(
                "board {board} has no intake column, so there is nowhere to put a new ticket. \
                 Add an open-category column in board settings → statuses."
            )));
        }
    };
    if let Some(parent) = input.parent_id.filter(|p| !p.is_empty()) {
        assert_valid_parent(pg, None, parent, input.board_id).await?;
    }
    if !input.tags.is_empty() {
        ensure_labels(pg, input.board_id, input.tags).await?;
    }
    // THE ONE DOOR, for the fourth write path (agent-writes.ts).
    // `create_ticket` is an MCP tool and its title and description are
    // agent-authored text on its way to a human — and, through
    // `indexTicket`, on its way back into another agent's context.
    // `created_by` is verified against agent_defs inside, so a person's
    // ticket is untouched.
    let mut guarded = [
        Some(input.title.to_string()),
        input.description.map(|d| d.to_string()),
    ];
    guard_agent_fields(
        pg,
        "ticket-write",
        WriteAuthor::Name(input.created_by),
        &mut guarded,
        None,
    )
    .await;
    let title = guarded[0]
        .clone()
        .unwrap_or_else(|| input.title.to_string());
    let description = guarded[1]
        .clone()
        .or_else(|| input.description.map(|d| d.to_string()));

    // Ticket ref and row in ONE transaction: the counter bump and the insert
    // share the fate, so a board can never burn a number into nothing.
    let mut tx = pg.begin().await?;
    let (ticket_no,): (i32,) = sqlx::query_as(
        "update boards set ticket_seq = ticket_seq + 1, updated_at = now() \
         where id = $1::uuid returning ticket_seq",
    )
    .bind(input.board_id)
    .fetch_one(&mut *tx)
    .await?;
    let (id,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, ticket_no, title, description, priority, effort, \
         assignees, due_date, start_date, color, estimated_hours, parent_id, tags, \
         created_by, status) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7, $8::timestamptz, $9::timestamptz, $10, \
         $11::numeric, $12::uuid, $13, $14, $15) returning id::text",
    )
    .bind(input.board_id)
    .bind(ticket_no)
    .bind(&title)
    .bind(&description)
    .bind(input.priority.unwrap_or("medium"))
    .bind(input.effort)
    .bind(serde_json::json!(assignees))
    .bind(input.due_date)
    .bind(input.start_date)
    .bind(input.color)
    .bind(input.estimated_hours)
    .bind(input.parent_id.filter(|p| !p.is_empty()))
    .bind(serde_json::json!(input.tags))
    .bind(input.created_by)
    .bind(&status)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    log_activity(pg, &id, input.created_by, "created", "created this task").await?;
    if !assignees.is_empty() {
        log_activity(
            pg,
            &id,
            input.created_by,
            "assigned",
            &format!("assigned to {}", assignees.join(", ")),
        )
        .await?;
    }
    let Some(task) = get_task(pg, &id).await? else {
        return Err(TaskError::Db(sqlx::Error::RowNotFound));
    };
    // Born assigned into an agent-start column → push the work to the
    // agents. Detached, as in TS: the row is the record, the push is a
    // delivery.
    spawn_dispatch(deps, task.clone(), None);
    {
        let notify = deps.notify.clone();
        let actor = input.created_by.to_string();
        let ticket_ref = task.ticket_ref.clone();
        let board_id = input.board_id.to_string();
        let title_for_note = title.clone();
        let id_for_href = id.clone();
        tokio::spawn(async move {
            notify_task_users(
                &notify,
                &human_assignee_ids(&assignees),
                &actor,
                &TaskNotification {
                    kind: "task-assigned".into(),
                    title: format!("Assigned: {title_for_note}"),
                    body: ticket_ref,
                    href: Some(format!("/boards/{board_id}/{id_for_href}")),
                },
            )
            .await;
        });
    }
    publish_board(
        &deps.realtime,
        input.board_id,
        &BoardEvent {
            kind_tag: "task",
            task_id: Some(id.clone()),
            deleted: None,
        },
    );
    Ok(task)
}

// ── Human in the loop ────────────────────────────────────────────────────────
// The product promise: agents create, triage and report, but assigning work
// and signing it off are a person's call. That invariant lives HERE rather
// than in the route, so every caller inherits it — an agent write is an
// agent write whether it arrives over PUT /api/tasks/:id or from a future
// tool.

/// tasks.ts TaskActor. `Platform` is Talaria itself (the QA judge, dispatch)
/// — trusted like a human, and named as itself in the activity log.
#[derive(Debug, Clone)]
pub struct TaskActor {
    pub kind: TaskActorKind,
    /// Activity/notification identity: an email for humans, a model id for
    /// agents, `judge:<model>` for the platform.
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskActorKind {
    Human,
    Agent,
    Platform,
}

impl TaskActor {
    pub fn human(id: impl Into<String>) -> Self {
        TaskActor {
            kind: TaskActorKind::Human,
            id: id.into(),
        }
    }
    pub fn agent(model: impl Into<String>) -> Self {
        TaskActor {
            kind: TaskActorKind::Agent,
            id: model.into(),
        }
    }
    pub fn platform(id: impl Into<String>) -> Self {
        TaskActor {
            kind: TaskActorKind::Platform,
            id: id.into(),
        }
    }
}

/// The system Blocked column (statuses.rs owns the key and refuses to let
/// any board mint another one), so a literal is the honest handle: no
/// board_statuses row carries the 'blocked' category, which is exactly why
/// it fell out of every key list in StatusMeta.
const BLOCKED_STATUS: &str = "blocked";

// ── THE agent-authority question, as ONE exported predicate ──────────────────
//
// ONE PREDICATE, MANY GATES. The patch gate, the work-session loop, the
// heartbeat and the MCP tool surface all ask the same question — may THIS
// agent act on THIS ticket — and before this consolidation they asked four
// private approximations of it that each omitted a clause. It returns the
// REASON rather than a boolean so the refusal is the same sentence wherever
// the write arrived, and a new caller cannot invent a vaguer one.

/// The ticket shape the predicate needs (TS AgentWriteTarget). `get_task`
/// returns it; so does the row the heartbeat query selects.
pub struct AgentWriteTarget {
    pub board_id: String,
    pub status: String,
    pub archived_at: Option<String>,
}

/// What the agent is trying to do (TS AgentIntent). The two verbs differ in
/// EXACTLY one clause, which is why they are one function and not two:
/// · `Write`   — change the ticket, or hang a record off it (status, fields,
///               dependency edges, spend rows, workbench jobs).
/// · `Comment` — say something on it. A CLOSED ticket still takes comments:
///               that is the agent's channel on work it can no longer edit,
///               and it is deliberate. Archival is not that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentIntent {
    Write,
    Comment,
}

/// Why an agent may not act on this ticket, or None when it may (TS
/// agentTicketRefusal).
///
/// FOUR STOP CONDITIONS, in the order a person would ask them:
/// · the BOARD does not allow this agent — revoked, never granted, or the
///   board is archived/gone;
/// · the ticket is ARCHIVED — a person took this work off the table;
/// · the intent is COMMENT — not a stop at all, the exemption above;
/// · the ticket is CLOSED — a person signed off on this work.
///
/// WHY ARCHIVAL STOPS COMMENTS TOO, at both levels. The comment exemption
/// exists for work "the agent can no longer edit" but a person is still
/// looking at — a signed-off ticket sitting in a done column on a live board.
/// Archival is not that: it withdraws the work from view. Board archival
/// already refused agent comments (it runs through `board_allows_agent`)
/// while ticket archival did not, which is one act of a person's meaning two
/// different things one level apart. Made consistent in the direction that
/// keeps the exemption's stated reason true: closed keeps its channel,
/// archived does not have one, at either level. READS are not this question —
/// an agent that can see the board can read the ticket, because reading
/// changes nothing.
///
/// TS's `facts` argument is a per-pass CACHE, never an answer; the Rust port
/// reads through, and the one board it resolves twice per call is a query the
/// cache existed to save.
pub async fn agent_ticket_refusal(
    pg: &PgPool,
    task: &AgentWriteTarget,
    agent: &AgentSubject,
    intent: AgentIntent,
) -> Result<Option<String>, sqlx::Error> {
    let board = board_info(pg, &task.board_id).await?;
    if !board_allows_agent(pg, &task.board_id, agent).await? {
        if board.exists && board.archived_at.is_some() {
            return Ok(Some(format!(
                "agents cannot work a ticket on an archived board — a person restores {} first",
                board.label
            )));
        }
        return Ok(Some(format!(
            "agent \"{}\" is not allowed on this board",
            subject_model(agent)
        )));
    }
    if task.archived_at.is_some() {
        return Ok(Some(
            "agents cannot work an archived ticket — a person restores it first".into(),
        ));
    }
    if intent == AgentIntent::Comment {
        return Ok(None);
    }
    let meta = status_meta(pg, &task.board_id).await?;
    if meta.terminal(&task.status) {
        return Ok(Some("agents cannot change a closed ticket".into()));
    }
    Ok(None)
}

impl From<&Task> for AgentWriteTarget {
    fn from(t: &Task) -> Self {
        AgentWriteTarget {
            board_id: t.board_id.clone(),
            status: t.status.clone(),
            archived_at: t.archived_at.clone(),
        }
    }
}

// A misconfigured board is an OPERATOR problem, and the only party that sees
// the API response is the agent that got refused. So log it too — on a slow
// cadence per board (same idiom as agent-auth's legacy warning): a heartbeat
// loop must not bury the log, but one line from whenever the server started
// can't answer "is this still happening?".
static BOARD_WARNED_AT: LazyLock<Mutex<HashMap<String, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const BOARD_WARN_EVERY_MS: i64 = 15 * 60 * 1000;
fn warn_board_config(board_id: &str, line: &str) {
    let now = now_ms();
    let mut warned = BOARD_WARNED_AT.lock().unwrap_or_else(|p| p.into_inner());
    if now - warned.get(board_id).copied().unwrap_or(0) < BOARD_WARN_EVERY_MS {
        return;
    }
    warned.insert(board_id.to_string(), now);
    tracing::warn!("[tasks] {line}");
}

/// tasks.ts boardLabel: board name for a diagnostic, falling back to the id.
/// Error paths only.
async fn board_label(pg: &PgPool, board_id: &str) -> Result<String, sqlx::Error> {
    Ok(board_info(pg, board_id).await?.label)
}

/// tasks.ts handoffTarget — where an agent's terminal move actually lands.
///
/// THE COERCION MUST NOT INVENT A DESTINATION. The destination is CHECKED,
/// not assumed: a real review-category column, not a done column, not an
/// agent-start pickup queue — all three hold by construction because
/// `review_key` is picked from `placeable` inside statusMeta. A non-None
/// `review_key` IS the answer. When there isn't one the write is REFUSED,
/// and the refusal names the board and the fix — a board an admin has to
/// correct must not read as an agent that misbehaved.
async fn handoff_target(pg: &PgPool, cur: &Task, meta: &StatusMeta) -> TaskResult<String> {
    if let Some(key) = &meta.review_key {
        return Ok(key.clone());
    }
    let board = board_label(pg, &cur.board_id).await?;
    // Review columns EXIST but not one of them can hold a hand-off: each is
    // either also an agent-start pickup queue (handing off would loop
    // straight back to the agent) or itself terminal (a review column
    // labelled "Cancelled"). Both are data that predates the
    // cross-validation — statuses.rs refuses to write it now — and both need
    // an admin, so say which switch to flip. Without this the write died on
    // the assignment gate as a bare "agents cannot assign tickets": true of
    // the symptom, useless about the cause.
    if !meta.review_keys.is_empty() {
        let cols: Vec<String> = meta
            .review_keys
            .iter()
            .map(|k| format!("\"{k}\""))
            .collect();
        let cols = cols.join(", ");
        let many = meta.review_keys.len() > 1;
        let line = format!(
            "board {board} has no usable review column: {cols} {} but {} also flagged \
             agent-start or itself terminal, so handing work off would drop it straight \
             back into the agent pickup queue (or close it). Clear \"agent start\" on {} \
             and rename {} whose key is \"failed\"/\"cancelled\" (board settings → \
             statuses), or add a review column that agents do not pick up from.",
            if many {
                "are review columns"
            } else {
                "is a review column"
            },
            if many { "each is" } else { "it is" },
            if many { "those columns" } else { "that column" },
            if many { "any" } else { "it" },
        );
        warn_board_config(&cur.board_id, &line);
        return Err(TaskError::Refusal(line));
    }
    let line = format!(
        "board {board} has no review column, so there is nowhere to hand work off — \
         a person has to close this ticket. Add a review-category column in board \
         settings → statuses."
    );
    warn_board_config(&cur.board_id, &line);
    Err(TaskError::Refusal(line))
}

// ── Update ───────────────────────────────────────────────────────────────────

/// tasks.ts TaskPatch. The tri-state fields (`Option<Option<T>>`) keep TS's
/// three meanings apart: absent (None — don't touch), present-null
/// (Some(None) — clear it), and a value (Some(Some(v))). Several of the
/// activity lines below exist precisely because "cleared" used to be
/// indistinguishable from "untouched".
#[derive(Debug, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub effort: Option<Option<String>>,
    pub assignees: Option<Vec<String>>,
    pub due_date: Option<Option<String>>,
    pub start_date: Option<Option<String>>,
    pub color: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub outcome: Option<Option<String>>,
    pub resolution: Option<Option<String>>,
    pub error_message: Option<Option<String>>,
    pub archived: Option<bool>,
    pub estimated_hours: Option<Option<f64>>,
    pub parent_id: Option<Option<String>>,
    /// Full replacement list of attachment chips (uploads + refs), already
    /// resolved/ACL-checked by the route — same shape as message
    /// attachments.
    pub attachments: Option<serde_json::Value>,
    /// Seconds to add to accumulated time-spent (agents report per
    /// iteration).
    pub add_time_spent_seconds: Option<f64>,
    /// WHY the column moved, when the reason is not visible from the move
    /// itself — appended to the one 'status' activity line this function
    /// writes. It exists so a caller with a reason does not have to become a
    /// second writer of `tasks.status` to record one: `delete_status`
    /// reassigns a doomed column's tickets through here and says so. Never a
    /// substitute for the move itself.
    pub status_note: Option<String>,
}

/// TS `pick(v, fallback)`: undefined → fallback, everything else (null
/// included) → the value.
fn pick<T: Clone>(v: &Option<Option<T>>, fallback: Option<T>) -> Option<T> {
    match v {
        None => fallback,
        Some(inner) => inner.clone(),
    }
}

/// tasks.ts agentSafePatch — the patch an agent is actually allowed to
/// apply: assignment, planning and archival stripped; terminal moves
/// redirected to the board's review catch. Throws (as TaskError) where a
/// weaker form would be a lie — assigning, reopening, taking work back out
/// of review — instead of quietly doing something else. The clauses below
/// are the whole invariant: a person assigns, a person signs off, a person
/// unblocks, and what a person signed off on stays put.
async fn agent_safe_patch(
    pg: &PgPool,
    cur: &Task,
    mut patch: TaskPatch,
    agent: &AgentSubject,
    meta: &StatusMeta,
) -> TaskResult<TaskPatch> {
    // ── PRESENCE, NOT TRUTHINESS ───────────────────────────────────────────
    // Every guard below asks `patch.status.is_some()`, never the string's
    // truth. `status: ''` is a legal JSON value and a falsy one, and the
    // TS guards that asked `patch.status && …` skipped the lot on it;
    // update_task rejects a status that is not a column on the board before
    // this function runs (so '' dies at the door, from any caller, agent or
    // not), and the route's schema carries min(1); this module states
    // presence explicitly so a third layer can't be the one that was
    // load-bearing.
    //
    // That, the board-level twin, the board's agent policy and the
    // closed-status rule are all `agent_ticket_refusal` now — the same
    // question the side doors, the heartbeat and both dispatch sides ask,
    // from the one definition above.
    let shut =
        agent_ticket_refusal(pg, &AgentWriteTarget::from(cur), agent, AgentIntent::Write).await?;
    if let Some(sentence) = shut {
        return Err(TaskError::ApprovalRequired(sentence));
    }
    let status_present_and_moving =
        |patch: &TaskPatch| patch.status.as_ref().is_some_and(|s| s != &cur.status);
    // A review column is the human sign-off QUEUE. Once an agent hands work
    // over, only a person — or the platform, whose judge bounces revisions
    // back — takes it out again. Otherwise the agent pulls its own work off
    // the reviewer's board and mints itself a fresh work session on every
    // lap.
    if meta.review_keys.contains(&cur.status) && status_present_and_moving(&patch) {
        return Err(TaskError::ApprovalRequired(
            "agents cannot take a ticket out of review — a person signs it off".into(),
        ));
    }
    // Blocked is a STOP SIGNAL raised for a person, and the dispatch prompt
    // sells it as one ("set status blocked … ends the session"). An agent
    // that can walk back out erases the signal the human was watching for
    // and, because work-dispatch treats every active column as working,
    // restarts its own session. Same rule as review: raising it is the
    // agent's call, clearing it is not.
    if cur.status == BLOCKED_STATUS && status_present_and_moving(&patch) {
        return Err(TaskError::ApprovalRequired(
            "agents cannot move a ticket out of blocked — a person unblocks it".into(),
        ));
    }
    // A ticket sitting in a status that is not a column on its board is
    // STRANDED (the column was deleted or recategorised under it). Nothing
    // above can class it — it is in no category — so an agent moving it
    // would be moving work nobody can see out of a state nobody chose. A
    // person places it.
    if status_present_and_moving(&patch) && !meta.keys.contains(&cur.status) {
        return Err(TaskError::ApprovalRequired(format!(
            "\"{}\" is no longer a column on this board — a person has to place this ticket",
            cur.status
        )));
    }
    // Assignment stays human; so do the planning fields (estimate, sub-task
    // structure) and archival, which hides work from the people watching it.
    patch.assignees = None;
    patch.estimated_hours = None;
    patch.parent_id = None;
    patch.archived = None;
    if patch.status.is_none() {
        return Ok(patch);
    }
    // Every terminal move lands in review instead — the board's done columns
    // AND the off-board keys. Reporting a result is the agent's job, closing
    // on it is not. handoff_target REFUSES rather than guessing when the
    // board has no column that can hold a hand-off. The result still FALLS
    // THROUGH to the assignment gate rather than returning: a coerced
    // destination is still a destination.
    if let Some(s) = patch.status.clone()
        && meta.terminal(&s)
    {
        let target = handoff_target(pg, cur, meta).await?;
        patch.status = Some(target);
    }
    // ENTERING an agent-start column is assignment — the destination is the
    // gate, whatever column the ticket came from and whether the agent named
    // that column or the coercion above picked it. Only a ticket already
    // sitting in one moves freely (an agent working what it was given).
    // Gating on the SOURCE let any intermediate status launder a
    // self-assignment in two writes, so a human dragging work back to intake
    // to stop an agent didn't actually stop it.
    if let Some(s) = &patch.status
        && meta.agent_start_keys.contains(s)
        && !meta.agent_start_keys.contains(&cur.status)
    {
        return Err(TaskError::ApprovalRequired(
            "agents cannot assign tickets".into(),
        ));
    }
    Ok(patch)
}

/// tasks.ts updateTask. THE write path: one UPDATE, then the activity
/// cascade, then the fan-outs. Returns the re-read task, or None when the id
/// names nothing (the route's 404).
pub async fn update_task(
    deps: &TaskDeps,
    id: &str,
    mut patch: TaskPatch,
    who: &TaskActor,
) -> TaskResult<Option<Task>> {
    let pg = &deps.pg;
    let Some(cur) = get_task(pg, id).await? else {
        return Ok(None);
    };
    let meta = status_meta(pg, &cur.board_id).await?;
    // PRESENCE, not truthiness — and this is the door every other status
    // rule stands behind. `status: ''` passed a truthiness check as absent
    // while the writer below treated it as present, so an agent could blank
    // a ticket's column with every guard in agent_safe_patch skipped. Asking
    // presence sends '' straight into the board-membership check, which no
    // board can satisfy: '' is not a column anywhere, so it is refused here,
    // once, for every caller — route, judge, workbench or a future tool.
    if let Some(s) = &patch.status
        && !meta
            .keys
            .iter()
            .map(|k| k.as_str())
            .chain(OFF_BOARD_STATUSES.iter().copied())
            .any(|k| k == s)
    {
        return Err(TaskError::Refusal(format!(
            "\"{s}\" is not a status on this board"
        )));
    }
    if who.kind == TaskActorKind::Agent {
        // `who.id` IS the agent's fleet model id (TaskActor documents it as
        // such), so the board-policy half of the invariant is answered from
        // the same actor every other clause is answered from.
        patch =
            agent_safe_patch(pg, &cur, patch, &AgentSubject::Model(who.id.clone()), &meta).await?;
        // Post-condition on the REWRITTEN patch (terminal moves land in the
        // review catch): whatever an agent write is about to store names a
        // column this board actually has. handoff_target guarantees the
        // category as well; this is belt to its braces: no path may store a
        // status the board cannot render.
        if let Some(s) = &patch.status
            && !meta.keys.contains(s)
        {
            return Err(TaskError::Refusal(format!(
                "\"{s}\" is not a status on this board"
            )));
        }
        // THE ONE DOOR, for the fifth write path (agent-writes.ts).
        // `triage_ticket` and `report_outcome` are MCP tools, so every
        // string below is a tool argument — model output that never touched
        // a harness. `indexTicket` re-indexes title+description on every
        // update, `notifyMentions` mails the description, and the judge
        // reads `outcome`/`resolution` straight into another model's prompt:
        // the outcome path is how an agent's own credential reached a
        // third-party judge endpoint with the guard having said so one
        // statement earlier and done nothing about it.
        let mut guarded = [
            patch.title.clone(),
            patch.description.clone().flatten(),
            patch.outcome.clone().flatten(),
            patch.resolution.clone().flatten(),
            patch.error_message.clone().flatten(),
        ];
        guard_agent_fields(
            pg,
            "ticket-write",
            WriteAuthor::Agent(&who.id),
            &mut guarded,
            None,
        )
        .await;
        patch.title = guarded[0].clone();
        patch.description = patch.description.as_ref().map(|_| guarded[1].clone());
        patch.outcome = patch.outcome.as_ref().map(|_| guarded[2].clone());
        patch.resolution = patch.resolution.as_ref().map(|_| guarded[3].clone());
        patch.error_message = patch.error_message.as_ref().map(|_| guarded[4].clone());
    }
    let assignees = patch
        .assignees
        .clone()
        .unwrap_or_else(|| cur.assignees.clone());
    let attachments = patch
        .attachments
        .clone()
        .unwrap_or_else(|| cur.attachments.clone());
    if let Some(Some(parent)) = patch.parent_id.as_ref()
        && !parent.is_empty()
    {
        assert_valid_parent(pg, Some(id), parent, &cur.board_id).await?;
    }
    if let Some(tags) = patch.tags.as_ref().filter(|t| !t.is_empty()) {
        ensure_labels(pg, &cur.board_id, tags).await?;
    }
    // Assignees on an intake ticket promote it into the pickup queue —
    // approval by assignment, so an agent's own write never triggers it. The
    // DESTINATION is never invented: with no usable agent-start column
    // (`assigned_key` None) there is no pickup queue to promote into, and
    // the ticket stays in intake rather than being moved somewhere that
    // merely sorted first — which, before `assigned_key` got this treatment,
    // could be a DONE column, so assigning someone closed the ticket.
    let promoted_to = if who.kind != TaskActorKind::Agent
        && !assignees.is_empty()
        && meta.default_key.as_deref() == Some(cur.status.as_str())
    {
        meta.assigned_key.clone()
    } else {
        None
    };
    let next_title = patch.title.clone().unwrap_or_else(|| cur.title.clone());
    let next_description = pick(&patch.description, cur.description.clone());
    let next_effort = pick(&patch.effort, cur.effort.clone());
    let next_priority = patch
        .priority
        .clone()
        .unwrap_or_else(|| cur.priority.clone());
    let next_due_date = pick(&patch.due_date, cur.due_date.clone());
    let next_start_date = pick(&patch.start_date, cur.start_date.clone());
    let next_color = pick(&patch.color, cur.color.clone());
    let next_estimated_hours = pick(&patch.estimated_hours, cur.estimated_hours);
    let next_parent_id = pick(&patch.parent_id, cur.parent_id.clone());
    let next_tags = patch.tags.clone().unwrap_or_else(|| cur.tags.clone());
    let next_outcome = pick(&patch.outcome, cur.outcome.clone());
    let next_resolution = pick(&patch.resolution, cur.resolution.clone());
    let next_error_message = pick(&patch.error_message, cur.error_message.clone());
    let next_status = patch
        .status
        .clone()
        .or(promoted_to)
        .unwrap_or_else(|| cur.status.clone());
    let completed_at = completed_at_for(&meta, &next_status, cur.completed_at.as_deref());
    let archived_at = match patch.archived {
        None => cur.archived_at.clone(),
        Some(true) => Some(
            cur.archived_at
                .clone()
                .unwrap_or_else(|| epoch_ms_to_iso(now_ms())),
        ),
        Some(false) => None,
    };
    let add_seconds = patch.add_time_spent_seconds.unwrap_or(0.0).round().max(0.0) as i64;

    sqlx::query(
        "update tasks set title=$2, description=$3, status=$4, priority=$5, effort=$6, \
         assignees=$7, due_date=$8::timestamptz, start_date=$9::timestamptz, color=$10, \
         estimated_hours=$11::numeric, parent_id=$12::uuid, tags=$13, attachments=$14, \
         outcome=$15, resolution=$16, error_message=$17, completed_at=$18::timestamptz, \
         archived_at=$19::timestamptz, \
         time_spent_seconds=time_spent_seconds + $20, updated_at=now() \
         where id=$1::uuid",
    )
    .bind(id)
    .bind(&next_title)
    .bind(&next_description)
    .bind(&next_status)
    .bind(&next_priority)
    .bind(&next_effort)
    .bind(serde_json::json!(assignees))
    .bind(&next_due_date)
    .bind(&next_start_date)
    .bind(&next_color)
    .bind(next_estimated_hours)
    .bind(&next_parent_id)
    .bind(serde_json::json!(next_tags))
    .bind(&attachments)
    .bind(&next_outcome)
    .bind(&next_resolution)
    .bind(&next_error_message)
    .bind(&completed_at)
    .bind(&archived_at)
    .bind(add_seconds)
    .execute(pg)
    .await?;

    if patch.archived.is_some() && patch.archived != Some(cur.archived_at.is_some()) {
        log_activity(
            pg,
            id,
            &who.id,
            "archived",
            if patch.archived == Some(true) {
                "archived this ticket"
            } else {
                "restored this ticket"
            },
        )
        .await?;
    }

    // THE COLUMN MOVED — however it moved. Gating this on
    // `patch.status.is_some()` meant PROMOTION BY ASSIGNMENT left nothing on
    // the record: a person patching only `assignees` moved the ticket out of
    // intake and into the agent pickup queue, and the ticket's own history
    // said only "assigned to …". The watchers were not told either.
    // `next_status` is what was actually written, so ask it.
    if next_status != cur.status {
        let why = patch.status_note.clone().unwrap_or_else(|| {
            if patch.status.is_none() {
                "promoted by assignment".to_string()
            } else {
                String::new()
            }
        });
        let moved = if why.is_empty() {
            format!("moved to {next_status}")
        } else {
            format!("moved to {next_status} ({why})")
        };
        log_activity(pg, id, &who.id, "status", &moved).await?;
        // Watchers + assigned humans hear about status moves (never the
        // actor), and `ticket_audience` re-reads board membership for both
        // halves at this moment — see its doc for why the write-time checks
        // are not enough. The audience read is awaited (TS awaits
        // ticketAudience before the void); the notification writes are
        // detached.
        let audience = ticket_audience(pg, id, &cur.board_id, &assignees).await?;
        {
            let notify = deps.notify.clone();
            let actor = who.id.clone();
            let title = format!(
                "{}: {}",
                cur.ticket_ref.clone().unwrap_or_else(|| cur.title.clone()),
                next_status.replacen('_', " ", 1)
            );
            let body = cur.title.clone();
            let href = format!("/boards/{}/{}", cur.board_id, id);
            tokio::spawn(async move {
                notify_task_users(
                    &notify,
                    &audience,
                    &actor,
                    &TaskNotification {
                        kind: "task-status".into(),
                        title,
                        body: Some(body),
                        href: Some(href),
                    },
                )
                .await;
            });
        }
    }
    // ── ONE push-side call ─────────────────────────────────────────────────
    // Two things mean "this is now someone's work": the ticket ENTERED a
    // pickup queue (a person moved the column, or promotion by assignment
    // did), and/or it GAINED agent assignees. Whether the ticket may be
    // dispatched to AT ALL is maybe_dispatch_ticket's call; this decides
    // only whether anything changed enough to ask.
    let added_agents = match &patch.assignees {
        Some(_) => agent_assignees(&assignees)
            .into_iter()
            .filter(|a| !agent_assignees(&cur.assignees).contains(a))
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };
    let entered_pickup = next_status != cur.status
        && meta.agent_start_keys.contains(&next_status)
        && !meta.agent_start_keys.contains(&cur.status);
    if entered_pickup || !added_agents.is_empty() {
        // Entering the queue is approval for EVERY agent assignee; a new
        // assignee on a ticket already sitting there gets only their own
        // push.
        let only = if entered_pickup {
            None
        } else {
            Some(added_agents)
        };
        spawn_dispatch_id(deps, id.to_string(), only);
    }
    if patch.assignees.is_some() && assignees != cur.assignees {
        let what = if assignees.is_empty() {
            "unassigned".to_string()
        } else {
            format!("assigned to {}", assignees.join(", "))
        };
        log_activity(pg, id, &who.id, "assigned", &what).await?;
        // NEWLY added humans get an inbox nudge.
        let added: Vec<String> = human_assignee_ids(&assignees)
            .into_iter()
            .filter(|uid| !human_assignee_ids(&cur.assignees).contains(uid))
            .collect();
        {
            let notify = deps.notify.clone();
            let actor = who.id.clone();
            let title = format!("Assigned: {}", cur.title);
            let body = cur.ticket_ref.clone();
            let href = format!("/boards/{}/{}", cur.board_id, id);
            tokio::spawn(async move {
                notify_task_users(
                    &notify,
                    &added,
                    &actor,
                    &TaskNotification {
                        kind: "task-assigned".into(),
                        title,
                        body,
                        href: Some(href),
                    },
                )
                .await;
            });
        }
    }
    if patch.priority.is_some() && patch.priority != Some(cur.priority.clone()) {
        log_activity(
            pg,
            id,
            &who.id,
            "priority",
            &format!("priority → {}", next_priority),
        )
        .await?;
    }
    if patch.effort.is_some() && next_effort != cur.effort {
        let what = if let Some(e) = &next_effort {
            format!("effort → {}", e.to_uppercase())
        } else {
            "effort cleared".to_string()
        };
        log_activity(pg, id, &who.id, "effort", &what).await?;
    }
    // `estimatedHours: 0` is a real estimate and a FALSY one — it used to be
    // stored as 0 and reported as "estimate cleared". None is the clear.
    // TS compares through `pgNum` because the row carries a string; the
    // Rust select casts numeric to float8 at the boundary, so this is the
    // same comparison a layer lower.
    if patch.estimated_hours.is_some() && next_estimated_hours != cur.estimated_hours {
        let what = if let Some(h) = next_estimated_hours {
            format!("estimate → {h}h")
        } else {
            "estimate cleared".to_string()
        };
        log_activity(pg, id, &who.id, "estimate", &what).await?;
    }
    if patch.parent_id.is_some() && next_parent_id != cur.parent_id {
        log_activity(
            pg,
            id,
            &who.id,
            "parent",
            if next_parent_id.is_some() {
                "made a sub-task"
            } else {
                "promoted to top level"
            },
        )
        .await?;
    }
    // ── The reported RESULT, and the labels ────────────────────────────────
    // Presence, not truthiness, again — and here the falsy value is the
    // DAMAGING one. A truthiness check meant an agent could BLANK the
    // outcome a reviewer was about to read (`outcome: ''`, or null) and
    // leave nothing on the record: the ticket sat in review with an empty
    // result and an activity log that still said "reported an outcome" from
    // the write before. Same for the rest of the result fields, which had no
    // activity line at all, and for tags, where `tags: []` cleared every
    // label silently.
    if patch.outcome.is_some() && next_outcome != cur.outcome {
        log_activity(
            pg,
            id,
            &who.id,
            "outcome",
            if next_outcome.is_some() {
                "reported an outcome"
            } else {
                "cleared the outcome"
            },
        )
        .await?;
    }
    if patch.resolution.is_some() && next_resolution != cur.resolution {
        log_activity(
            pg,
            id,
            &who.id,
            "resolution",
            if next_resolution.is_some() {
                "wrote a resolution"
            } else {
                "cleared the resolution"
            },
        )
        .await?;
    }
    if patch.error_message.is_some() && next_error_message != cur.error_message {
        log_activity(
            pg,
            id,
            &who.id,
            "error",
            if next_error_message.is_some() {
                "reported an error"
            } else {
                "cleared the error"
            },
        )
        .await?;
    }
    if patch.tags.is_some() && next_tags != cur.tags {
        let what = if next_tags.is_empty() {
            "cleared every label".to_string()
        } else {
            format!("labels → {}", next_tags.join(", "))
        };
        log_activity(pg, id, &who.id, "tags", &what).await?;
    }
    // Length told you nothing about a 1-for-1 swap: replacing the evidence on
    // a ticket in review was invisible. Compare the LIST. (serde_json
    // compares Values structurally where TS compares JSON.stringify text —
    // the verdicts differ only when key ORDER alone differs, which no chip's
    // meaning rides on.)
    if patch.attachments.is_some() && attachments != cur.attachments {
        let cur_len = cur.attachments.as_array().map(|a| a.len()).unwrap_or(0);
        let new_len = attachments.as_array().map(|a| a.len()).unwrap_or(0);
        let what = if new_len > cur_len {
            format!("attached {} file(s)", new_len - cur_len)
        } else if new_len < cur_len {
            "removed an attachment".to_string()
        } else {
            "replaced an attachment".to_string()
        };
        log_activity(pg, id, &who.id, "attachments", &what).await?;
    }
    publish_board(
        &deps.realtime,
        &cur.board_id,
        &BoardEvent {
            kind_tag: "task",
            task_id: Some(id.to_string()),
            deleted: None,
        },
    );
    get_task(pg, id).await.map_err(TaskError::Db)
}

/// The detached dispatch leg shared by the three writers that can hand an
/// agent work (create hands the task it already holds; the update/review
/// paths re-read fresh, exactly as TS does inside its void IIFE).
fn spawn_dispatch(deps: &TaskDeps, task: Task, only_agents: Option<Vec<String>>) {
    let Some(dispatch) = deps.dispatch.clone() else {
        return;
    };
    let pg = deps.pg.clone();
    tokio::spawn(async move {
        let only = only_agents.as_deref();
        maybe_dispatch_ticket(&pg, &dispatch, &dispatch_ticket_of(&task), only).await;
    });
}

fn spawn_dispatch_id(deps: &TaskDeps, id: String, only_agents: Option<Vec<String>>) {
    let Some(dispatch) = deps.dispatch.clone() else {
        return;
    };
    let pg = deps.pg.clone();
    tokio::spawn(async move {
        // TS re-reads the ticket inside the detached block: the push decides
        // on the row as it stands now, not the caller's stale copy.
        if let Ok(Some(fresh)) = get_task(&pg, &id).await {
            maybe_dispatch_ticket(
                &pg,
                &dispatch,
                &dispatch_ticket_of(&fresh),
                only_agents.as_deref(),
            )
            .await;
        }
    });
}

/// tasks.ts deleteTask. Hard delete — comments, watchers, activity and
/// dependencies ride the foreign keys.
pub async fn delete_task(deps: &TaskDeps, id: &str) -> Result<(), sqlx::Error> {
    let board_id = task_board_id(&deps.pg, id).await?;
    sqlx::query("delete from tasks where id = $1::uuid")
        .bind(id)
        .execute(&deps.pg)
        .await?;
    if let Some(board_id) = board_id {
        publish_board(
            &deps.realtime,
            &board_id,
            &BoardEvent {
                kind_tag: "task",
                task_id: Some(id.to_string()),
                deleted: Some(true),
            },
        );
    }
    Ok(())
}

// ── Comments ─────────────────────────────────────────────────────────────────

/// TS TaskComment.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskComment {
    pub id: String,
    pub author: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub created_at: String,
}

type CommentRow = (String, String, String, Option<String>, i64);

fn comment_of(r: CommentRow) -> TaskComment {
    let (id, author, content, parent_id, created_ms) = r;
    TaskComment {
        id,
        author,
        content,
        parent_id,
        created_at: epoch_ms_to_iso(created_ms),
    }
}

pub async fn list_comments(pg: &PgPool, task_id: &str) -> Result<Vec<TaskComment>, sqlx::Error> {
    let rows: Vec<CommentRow> = sqlx::query_as(
        "select id::text, author, content, parent_id::text, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from task_comments where task_id = $1::uuid order by created_at asc",
    )
    .bind(task_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(comment_of).collect())
}

/// tasks.ts addComment — THE comment write, and therefore the place the
/// guard on agent-authored comments lives. `mcp comment` reaches here as a
/// tool ARGUMENT — model output that never touched a harness and, until
/// this call existed, never touched a guard either. `guard_agent_write`
/// decides whether this author is an agent at all (a person's comment is
/// not model output and is left alone), records what the gate-safe rules
/// find against that agent, and in strict mode hands back a redacted body.
/// See agent-writes.rs for why a credential is redacted rather than
/// blocked.
///
/// Inside the write rather than at the route on purpose: `POST
/// /api/tasks/:id/comments` is not the only caller — the workbench posts an
/// agent's plan comment through here too — and a guard at one caller is a
/// guard the next caller does not have.
pub async fn add_comment(
    pg: &PgPool,
    task_id: &str,
    author: &str,
    content: &str,
    parent_id: Option<&str>,
) -> Result<TaskComment, sqlx::Error> {
    let guarded = guard_agent_write(
        pg,
        "ticket-comment",
        WriteAuthor::Name(author),
        content,
        None,
    )
    .await;
    let row: CommentRow = sqlx::query_as(
        "insert into task_comments (task_id, author, content, parent_id) \
         values ($1::uuid, $2, $3, $4::uuid) \
         returning id::text, author, content, parent_id::text, \
                (trunc(extract(epoch from created_at) * 1000))::bigint",
    )
    .bind(task_id)
    .bind(author)
    .bind(&guarded.text)
    .bind(parent_id)
    .fetch_one(pg)
    .await?;
    log_activity(pg, task_id, author, "comment", "commented").await?;
    Ok(comment_of(row))
}

// ── Watchers ─────────────────────────────────────────────────────────────────
// A WATCHER IS AN AUDIENCE, and it was the one audience on a ticket that
// nothing in the product ever declared. `task_watchers.watcher` was free
// text: POST /api/tasks/:id/watchers took any non-empty string and inserted
// it, and the fan-out resolved anything containing '@' to a user id by
// email. Nobody asked whether that person could see the board. A user who
// was a member of NO board got "ALPH-1: in progress | <title> |
// /boards/<id>/<id>" in her inbox AND as SMTP bytes, followed by a 403 from
// the link she was sent, and could not unsubscribe herself because DELETE
// carried the same membership guard she was failing.
//
// The contrast is what makes it a defect rather than a design choice: a
// ticket ASSIGNEE is checked against board membership by
// `invalid_assignee` on both write routes. A watcher is the same fan-out
// with the check missing.
//
// So: ONE resolution of "who is watching this ticket", below, and every
// reader goes through it — `list_watchers` (the UI, the API response) and
// `ticket_audience` (the notifications). There is no path that reads
// `task_watchers` without asking it.
//
// WHAT HAPPENS TO A STORED WATCHER WHO NO LONGER QUALIFIES — the rows
// written before this check existed, and anyone whose board access is later
// revoked: the row is KEPT and the person is absent from both answers. Not
// shown in the ticket's watcher list, not notified, not mailed. The
// alternative shapes were both worse. Dropping them at send time while
// `list_watchers` still returned them is a lie in the UI — the panel names
// someone who is being told nothing — and that is the bug this replaces,
// one layer down. Deleting the row on read destroys a subscription on a
// read path, and makes a TEMPORARY removal from a board (or an accidental
// one) permanently unsubscribe someone with no record that it happened.
// Kept-but-dormant means restoring board access restores what they asked
// for, and `remove_watcher` is still the way to end it for good.

/// Stored watcher strings → the accounts they name. Email match, case- and
/// whitespace-insensitive; a string that is not an email, or that names
/// nobody with an account here, is simply absent from the map.
async fn watcher_accounts(
    pg: &PgPool,
    watchers: &[String],
) -> Result<HashMap<String, (String, String)>, sqlx::Error> {
    let mut emails: Vec<String> = Vec::new();
    for w in watchers
        .iter()
        .filter(|w| w.contains('@'))
        .map(|w| w.trim().to_lowercase())
    {
        if !emails.iter().any(|e| e == &w) {
            emails.push(w);
        }
    }
    let mut out: HashMap<String, (String, String)> = HashMap::new();
    if emails.is_empty() {
        return Ok(out);
    }
    let rows: Vec<(String, String)> =
        sqlx::query_as("select id::text, email from users where lower(email) = any($1)")
            .bind(emails)
            .fetch_all(pg)
            .await?;
    let mut by_email: HashMap<String, (String, String)> = HashMap::new();
    for (id, email) in rows {
        by_email.insert(email.trim().to_lowercase(), (id, email));
    }
    for w in watchers {
        if let Some(account) = by_email.get(&w.trim().to_lowercase()) {
            out.insert(w.clone(), account.clone());
        }
    }
    Ok(out)
}

struct ResolvedWatcher {
    watcher: String,
    user_id: String,
}

/// The watchers of a ticket who can still SEE the ticket, in stored order.
///
/// Membership is asked through `board_role` one watcher at a time rather
/// than joined in SQL on purpose: `board_role` is the definition of "may
/// this person see this board" and it already covers the two ways of
/// holding one (a board_members row, or a team that owns the board). A join
/// here would be a third hand-written copy of that rule, which is the shape
/// this codebase keeps paying for. Tickets carry a handful of watchers;
/// this is not a hot path.
async fn resolve_watchers(
    pg: &PgPool,
    task_id: &str,
    board_id: Option<&str>,
) -> Result<Vec<ResolvedWatcher>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select watcher from task_watchers where task_id = $1::uuid order by created_at asc",
    )
    .bind(task_id)
    .fetch_all(pg)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let board = match board_id {
        Some(b) => Some(b.to_string()),
        None => task_board_id(pg, task_id).await?,
    };
    let Some(board) = board else {
        return Ok(Vec::new());
    };
    let stored: Vec<String> = rows.into_iter().map(|(w,)| w).collect();
    let accounts = watcher_accounts(pg, &stored).await?;
    let mut out = Vec::new();
    for w in stored {
        let Some((user_id, _)) = accounts.get(&w) else {
            continue;
        };
        if board_role(pg, user_id, &board).await?.is_none() {
            continue;
        }
        out.push(ResolvedWatcher {
            watcher: w,
            user_id: user_id.clone(),
        });
    }
    Ok(out)
}

/// The ticket's watchers as the product should show them: exactly the people
/// who will be notified.
pub async fn list_watchers(pg: &PgPool, task_id: &str) -> Result<Vec<String>, sqlx::Error> {
    Ok(resolve_watchers(pg, task_id, None)
        .await?
        .into_iter()
        .map(|w| w.watcher)
        .collect())
}

/// addWatcher's two answers: followed, or refused with the reason the
/// caller shows.
pub enum WatchOutcome {
    Added,
    Refused(String),
}

/// Follow a ticket. Refuses anyone who cannot already see it — the same
/// question `invalid_assignee` asks of an assignee, with the same answer
/// shape, so the reason reaches the caller instead of a bare 403.
///
/// Stores the account's OWN email rather than the string that was typed, so
/// case variants cannot become two rows for one person and the stored value
/// is always something `watcher_accounts` can resolve.
pub async fn add_watcher(
    pg: &PgPool,
    task_id: &str,
    watcher: &str,
) -> Result<WatchOutcome, sqlx::Error> {
    let Some(board_id) = task_board_id(pg, task_id).await? else {
        return Ok(WatchOutcome::Refused("no such ticket".into()));
    };
    let Some((user_id, email)) = watcher_accounts(pg, &[watcher.to_string()])
        .await?
        .get(watcher)
        .cloned()
    else {
        return Ok(WatchOutcome::Refused(
            "a watcher is a person with an account here — pass the email address they sign in with"
                .into(),
        ));
    };
    if board_role(pg, &user_id, &board_id).await?.is_none() {
        return Ok(WatchOutcome::Refused(
            "watchers must be members of this board".into(),
        ));
    }
    sqlx::query(
        "insert into task_watchers (task_id, watcher) values ($1::uuid, $2) \
         on conflict do nothing",
    )
    .bind(task_id)
    .bind(&email)
    .execute(pg)
    .await?;
    Ok(WatchOutcome::Added)
}

/// Unfollow. Case-insensitive on purpose: this is the escape hatch a person
/// reaches for when mail is arriving about a board they cannot open, and it
/// must not turn on whether their address was stored the way they typed it.
pub async fn remove_watcher(pg: &PgPool, task_id: &str, watcher: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from task_watchers where task_id = $1::uuid and lower(watcher) = $2")
        .bind(task_id)
        .bind(watcher.trim().to_lowercase())
        .execute(pg)
        .await?;
    Ok(())
}

// ── Quality reviews (approval gate) ──────────────────────────────────────────

/// TS QualityReview.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReview {
    pub id: String,
    pub reviewer: String,
    pub status: String,
    pub notes: Option<String>,
    pub created_at: String,
}

pub async fn list_reviews(pg: &PgPool, task_id: &str) -> Result<Vec<QualityReview>, sqlx::Error> {
    let rows: Vec<(String, String, String, Option<String>, i64)> = sqlx::query_as(
        "select id::text, reviewer, status, notes, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from quality_reviews where task_id = $1::uuid order by created_at desc",
    )
    .bind(task_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, reviewer, status, notes, created_ms)| QualityReview {
            id,
            reviewer,
            status,
            notes,
            created_at: epoch_ms_to_iso(created_ms),
        })
        .collect())
}

pub async fn add_review(
    deps: &TaskDeps,
    task_id: &str,
    reviewer: &str,
    status: &str,
    notes: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into quality_reviews (task_id, reviewer, status, notes) \
         values ($1::uuid, $2, $3, $4)",
    )
    .bind(task_id)
    .bind(reviewer)
    .bind(status)
    .bind(notes)
    .execute(&deps.pg)
    .await?;
    log_activity(
        &deps.pg,
        task_id,
        reviewer,
        "review",
        if status == "approved" {
            "approved this task"
        } else {
            "requested changes"
        },
    )
    .await?;
    if let Some(board_id) = task_board_id(&deps.pg, task_id).await? {
        publish_board(
            &deps.realtime,
            &board_id,
            &BoardEvent {
                kind_tag: "task",
                task_id: Some(task_id.to_string()),
                deleted: None,
            },
        );
    }
    Ok(())
}

/// When does a status stamp `completed_at`?
///
/// This is the NARROW question — "is this a done-CATEGORY column on this
/// board" — and deliberately NOT `meta.terminal()`, which also covers the
/// off-board keys: a `cancelled` or `failed` ticket was never completed, so
/// it must not carry a completion time. The two questions differ by exactly
/// those keys and every place that has confused them so far was a bug, so
/// the one place that answers it is this function.
fn completed_at_for(meta: &StatusMeta, status: &str, previous: Option<&str>) -> Option<String> {
    if meta.done_keys.iter().any(|k| k == status) {
        Some(
            previous
                .map(|p| p.to_string())
                .unwrap_or_else(|| epoch_ms_to_iso(now_ms())),
        )
    } else {
        None
    }
}

/// tasks.ts completeQualityReview — the reviewer's sign-off, moving the
/// ticket out of review in the same transaction that records the review.
/// None when the ticket is gone or no longer sitting in review (the
/// CAS-where makes the second a race resolving to nothing).
pub async fn complete_quality_review(
    deps: &TaskDeps,
    task_id: &str,
    reviewer: &str,
    review_status: &str,
    next_status: &str,
) -> TaskResult<Option<Task>> {
    let pg = &deps.pg;
    let Some(current) = get_task(pg, task_id).await? else {
        return Ok(None);
    };
    let meta = status_meta(pg, &current.board_id).await?;
    if !meta.review_keys.contains(&current.status) {
        return Ok(None);
    }
    if !meta.keys.iter().any(|k| k == next_status) {
        return Err(TaskError::Refusal(format!(
            "\"{next_status}\" is not a status on this board"
        )));
    }
    let completed_at = completed_at_for(&meta, next_status, current.completed_at.as_deref());
    let mut tx = pg.begin().await?;
    let rows: Vec<(String,)> = sqlx::query_as(
        "update tasks set status = $1, completed_at = $2::timestamptz, updated_at = now() \
         where id = $3::uuid and status = $4 returning id::text",
    )
    .bind(next_status)
    .bind(&completed_at)
    .bind(task_id)
    .bind(&current.status)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        // The ticket moved between the read and the write — TS's begin
        // callback returns false on exactly this and commits an empty
        // transaction; rollback of an unwritten one is the same end state.
        tx.rollback().await?;
        return Ok(None);
    }
    sqlx::query(
        "insert into quality_reviews (task_id, reviewer, status) values ($1::uuid, $2, $3)",
    )
    .bind(task_id)
    .bind(reviewer)
    .bind(review_status)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "insert into task_activity (task_id, actor, type, description) values \
         ($1::uuid, $2, 'review', $3), ($1::uuid, $2, 'status', $4)",
    )
    .bind(task_id)
    .bind(reviewer)
    .bind(if review_status == "approved" {
        "approved this task"
    } else {
        "requested changes"
    })
    .bind(format!("moved to {next_status}"))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    if meta.agent_start_keys.iter().any(|k| k == next_status)
        && !meta.agent_start_keys.contains(&current.status)
    {
        spawn_dispatch_id(deps, task_id.to_string(), None);
    }
    // Same audience question as the status move in update_task, same one
    // answer.
    let audience = ticket_audience(pg, task_id, &current.board_id, &current.assignees).await?;
    {
        let notify = deps.notify.clone();
        let actor = reviewer.to_string();
        let title = format!(
            "{}: {}",
            current
                .ticket_ref
                .clone()
                .unwrap_or_else(|| current.title.clone()),
            next_status.replacen('_', " ", 1)
        );
        let body = current.title.clone();
        let href = format!("/boards/{}/{}", current.board_id, task_id);
        tokio::spawn(async move {
            notify_task_users(
                &notify,
                &audience,
                &actor,
                &TaskNotification {
                    kind: "task-status".into(),
                    title,
                    body: Some(body),
                    href: Some(href),
                },
            )
            .await;
        });
    }
    publish_board(
        &deps.realtime,
        &current.board_id,
        &BoardEvent {
            kind_tag: "task",
            task_id: Some(task_id.to_string()),
            deleted: None,
        },
    );
    Ok(get_task(pg, task_id).await?)
}

// ── Dependencies (blocked-by / blocks) ───────────────────────────────────────

/// Returns [blocked-by, blocks] — the tickets this task depends on, and the
/// tickets that depend on this task.
pub async fn list_dependencies(
    pg: &PgPool,
    task_id: &str,
) -> Result<(Vec<TaskLink>, Vec<TaskLink>), sqlx::Error> {
    // AssertSqlSafe: the interpolation is this module's LINK_SELECT.
    let blocked_sql = format!(
        "{LINK_SELECT} join task_dependencies d on d.depends_on_id = t.id \
         where d.task_id = $1::uuid order by t.ticket_no"
    );
    let blocks_sql = format!(
        "{LINK_SELECT} join task_dependencies d on d.task_id = t.id \
         where d.depends_on_id = $1::uuid order by t.ticket_no"
    );
    let blocked_by: Vec<(String, Option<String>, String, String)> =
        sqlx::query_as(sqlx::AssertSqlSafe(blocked_sql.as_str()))
            .bind(task_id)
            .fetch_all(pg)
            .await?;
    let blocks: Vec<(String, Option<String>, String, String)> =
        sqlx::query_as(sqlx::AssertSqlSafe(blocks_sql.as_str()))
            .bind(task_id)
            .fetch_all(pg)
            .await?;
    let link =
        |(id, ticket_ref, title, status): (String, Option<String>, String, String)| TaskLink {
            id,
            ticket_ref,
            title,
            status,
        };
    Ok((
        blocked_by.into_iter().map(link).collect(),
        blocks.into_iter().map(link).collect(),
    ))
}

/// Adding `task → depends_on` closes a cycle iff `depends_on` already
/// reaches `task` through the existing edges. Self-edges are the length-1
/// case and were the ONLY case this guarded, so an agent could write X
/// blocked-by Y and then Y blocked-by X — a dependency graph no ticket in it
/// can ever satisfy, with nothing in the product able to tell the operator
/// why nothing unblocks. One recursive walk, on the write, is the cheap
/// place to say no.
async fn would_cycle(pg: &PgPool, task_id: &str, depends_on_id: &str) -> Result<bool, sqlx::Error> {
    if task_id == depends_on_id {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "with recursive reach(id) as ( \
           select depends_on_id from task_dependencies where task_id = $1::uuid \
           union \
           select d.depends_on_id from task_dependencies d join reach r on d.task_id = r.id \
         ) select 1 from reach where id = $2::uuid limit 1",
    )
    .bind(depends_on_id)
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

pub async fn add_dependency(
    deps: &TaskDeps,
    task_id: &str,
    depends_on_id: &str,
    actor: &str,
) -> TaskResult<()> {
    if would_cycle(&deps.pg, task_id, depends_on_id).await? {
        return Err(TaskError::Refusal(
            if task_id == depends_on_id {
                "a ticket cannot block itself"
            } else {
                "that would make the two tickets block each other — neither could ever be unblocked"
            }
            .into(),
        ));
    }
    sqlx::query(
        "insert into task_dependencies (task_id, depends_on_id) values ($1::uuid, $2::uuid) \
         on conflict do nothing",
    )
    .bind(task_id)
    .bind(depends_on_id)
    .execute(&deps.pg)
    .await?;
    log_activity(&deps.pg, task_id, actor, "dependency", "added a dependency").await?;
    if let Some(board_id) = task_board_id(&deps.pg, task_id).await? {
        publish_board(
            &deps.realtime,
            &board_id,
            &BoardEvent {
                kind_tag: "task",
                task_id: Some(task_id.to_string()),
                deleted: None,
            },
        );
    }
    Ok(())
}

pub async fn remove_dependency(
    deps: &TaskDeps,
    task_id: &str,
    depends_on_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from task_dependencies where task_id = $1::uuid and depends_on_id = $2::uuid",
    )
    .bind(task_id)
    .bind(depends_on_id)
    .execute(&deps.pg)
    .await?;
    if let Some(board_id) = task_board_id(&deps.pg, task_id).await? {
        publish_board(
            &deps.realtime,
            &board_id,
            &BoardEvent {
                kind_tag: "task",
                task_id: Some(task_id.to_string()),
                deleted: None,
            },
        );
    }
    Ok(())
}

// ── Activity ─────────────────────────────────────────────────────────────────

pub async fn log_activity(
    pg: &PgPool,
    task_id: &str,
    actor: &str,
    kind: &str,
    description: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into task_activity (task_id, actor, type, description) \
         values ($1::uuid, $2, $3, $4)",
    )
    .bind(task_id)
    .bind(actor)
    .bind(kind)
    .bind(description)
    .execute(pg)
    .await?;
    Ok(())
}

/// TS TaskActivity.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskActivity {
    pub id: String,
    pub actor: String,
    #[serde(rename = "type")]
    pub kind_tag: String,
    pub description: String,
    pub created_at: String,
}

pub async fn list_activity(pg: &PgPool, task_id: &str) -> Result<Vec<TaskActivity>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "select id::text, actor, type, description, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from task_activity where task_id = $1::uuid order by created_at desc limit 100",
    )
    .bind(task_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, actor, kind_tag, description, created_ms)| TaskActivity {
                id,
                actor,
                kind_tag,
                description,
                created_at: epoch_ms_to_iso(created_ms),
            },
        )
        .collect())
}

// ── The pull side ────────────────────────────────────────────────────────────

/// One servable ticket on the heartbeat's pull channel, with the workflows
/// matched to it riding along (plugin-side dispatch).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignedWork {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub board_id: String,
    pub workflows: Vec<crate::workflows::WorkflowDelivery>,
}

/// tasks.ts assignedWork — work assigned to an agent (by name), across all
/// boards, for the heartbeat.
///
/// THE PULL SIDE ASKS THE SAME QUESTIONS THE WRITE SIDE ASKS. Both of them,
/// and neither in its own words. The history of this one query is the whole
/// lesson: it once selected on `agent_start` alone, so it handed an agent
/// work every write route would then refuse; then it asked only
/// "closed to agents", which said nothing about the board's AGENT POLICY,
/// so a board owner revoking a grant 403'd every write while the heartbeat
/// kept serving; and the SQL still carried a status predicate of its own,
/// which DISAGREED with `working_keys` — an agent that obeyed step 2 of its
/// own dispatch prompt and moved the ticket into the board's active column
/// fell off the heartbeat mid-session. So the SQL states NO rule. It
/// narrows by ASSIGNMENT, and by a (board, key) pair list built from
/// `working_keys` itself — the SQL is a projection of the one predicate,
/// not a rival to it.
pub async fn assigned_work(
    pg: &PgPool,
    agent_name: &str,
) -> Result<Vec<AssignedWork>, sqlx::Error> {
    let subject = AgentSubject::Model(agent_name.to_string());
    let boards: Vec<(String,)> =
        sqlx::query_as("select distinct board_id::text from tasks where assignees @> $1::jsonb")
            .bind(serde_json::json!([agent_name]))
            .fetch_all(pg)
            .await?;
    // ONE pass over board facts for the whole heartbeat — columns, archival
    // AND agent policy, each resolved once per distinct board rather than
    // once per ticket. A board the agent is not allowed on is dropped HERE,
    // before its tickets are ever fetched.
    let mut pair_boards: Vec<String> = Vec::new();
    let mut pair_keys: Vec<String> = Vec::new();
    for (board_id,) in &boards {
        if !board_allows_agent(pg, board_id, &subject).await? {
            continue;
        }
        let meta = status_meta(pg, board_id).await?;
        for key in &meta.working_keys {
            pair_boards.push(board_id.clone());
            pair_keys.push(key.clone());
        }
    }
    if pair_boards.is_empty() {
        return Ok(Vec::new());
    }
    // The narrowed row: assignment plus the working-key join, before the
    // authority question is re-asked per ticket below.
    type WorkRow = (
        String,
        String,
        Option<String>,
        serde_json::Value,
        String,
        String,
        Option<i64>,
    );
    let rows: Vec<WorkRow> = sqlx::query_as(
        "select t.id::text, t.title, t.description, t.tags, t.board_id::text, t.status, \
                    (trunc(extract(epoch from t.archived_at) * 1000))::bigint \
             from tasks t \
             join unnest($1::text[], $2::text[]) as w(board_id, status) \
               on w.board_id = t.board_id::text and w.status = t.status \
             where t.assignees @> $3::jsonb \
             order by t.created_at asc",
    )
    .bind(&pair_boards)
    .bind(&pair_keys)
    .bind(serde_json::json!([agent_name]))
    .fetch_all(pg)
    .await?;
    // The pairs above already answered "is this column in play?"; this is
    // the TICKET half of the same authority question — its own archival,
    // and the board's, re-asked from the one predicate rather than restated
    // in SQL.
    let mut servable = Vec::new();
    for (id, title, description, tags, board_id, status, archived_ms) in rows {
        let target = AgentWriteTarget {
            board_id: board_id.clone(),
            status,
            archived_at: archived_ms.map(epoch_ms_to_iso),
        };
        if agent_ticket_refusal(pg, &target, &subject, AgentIntent::Write)
            .await?
            .is_some()
        {
            continue;
        }
        servable.push((id, title, description, json_strings(&tags), board_id));
    }
    // Matched workflows ride with the pull channel too (plugin-side
    // dispatch). ONE read of the workflow list for the whole heartbeat:
    // calling `workflows_for_task` per ticket re-read the table each time.
    // Workflows are org-wide, not board-scoped, so there is nothing to key a
    // cache by; the list is read once and handed to `workflows_from`, which
    // is the SAME match the one-off path uses. Spelling the filter out here
    // instead would make this the round's own counter-example: a second
    // expression of a rule, free to drift.
    let flows = if servable.is_empty() {
        Vec::new()
    } else {
        crate::workflows::list_workflows(pg).await?
    };
    Ok(servable
        .into_iter()
        .map(|(id, title, description, tags, board_id)| {
            let target = crate::workflows::MatchTarget {
                title: &title,
                description: description.as_deref(),
                tags: &tags,
                board_id: &board_id,
            };
            let workflows = crate::workflows::workflows_from(&flows, &target);
            AssignedWork {
                id,
                title,
                description,
                tags,
                board_id,
                workflows,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_assignees_strip_their_prefix_and_leave_agent_ids_alone() {
        let assignees: Vec<String> = ["user:u-1", "claude-opus-4-5", "user:u-2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(human_assignee_ids(&assignees), ["u-1", "u-2"]);
        // The `@>` predicates keep matching bare strings, so an agent id that
        // merely CONTAINS 'user' is not a human.
        assert!(!is_human_assignee("impersonating-user:x"));
        assert!(human_assignee_ids(&[]).is_empty());
    }

    // ── pick: presence, not truthiness ──────────────────────────────────────

    #[test]
    fn pick_keeps_absent_clear_and_value_apart() {
        let cur = Some("keep me".to_string());
        // Absent → the row's value.
        assert_eq!(pick(&None, cur.clone()), cur);
        // Present-null (the explicit "clear") is a VALUE, not a fallback.
        assert_eq!(pick(&Some(None), cur.clone()), None);
        // Present value wins.
        assert_eq!(
            pick(&Some(Some("new".into())), cur.clone()),
            Some("new".into())
        );
        // The empty string is present too — the falsy-string bug this shape
        // exists to prevent.
        assert_eq!(pick(&Some(Some(String::new())), None), Some(String::new()));
    }

    // ── completed_at_for: the narrow done-column question ───────────────────

    fn meta_with_done(done: &[&str]) -> StatusMeta {
        StatusMeta {
            keys: vec!["inbox".into(), "done".into()],
            agent_start_keys: vec![],
            review_key: None,
            review_keys: vec![],
            done_keys: done.iter().map(|k| k.to_string()).collect(),
            default_key: Some("inbox".into()),
            assigned_key: None,
            pickup_keys: vec![],
            working_keys: vec![],
            active_key: None,
        }
    }

    #[test]
    fn completed_at_stamps_done_columns_only() {
        let meta = meta_with_done(&["shipped"]);
        // A done column stamps now when there was nothing to keep.
        assert!(completed_at_for(&meta, "shipped", None).is_some());
        // …and KEEPS the previous stamp rather than re-stamping.
        assert_eq!(
            completed_at_for(&meta, "shipped", Some("2026-01-01T00:00:00.000Z")),
            Some("2026-01-01T00:00:00.000Z".into())
        );
        // A non-done column clears it — the ticket is no longer finished.
        assert_eq!(
            completed_at_for(&meta, "inbox", Some("2026-01-01T00:00:00.000Z")),
            None
        );
    }

    #[test]
    fn off_board_terminal_keys_never_carry_a_completion_time() {
        let meta = meta_with_done(&["shipped"]);
        // `cancelled` is terminal everywhere (OFF_BOARD_STATUSES) but was
        // never COMPLETED — meta.terminal() is the wrong question here, and
        // this is the one function allowed to answer the right one.
        assert_eq!(
            completed_at_for(&meta, "cancelled", Some("2026-01-01T00:00:00.000Z")),
            None
        );
        assert!(meta.terminal("cancelled"));
    }

    // ── TaskError: the two throw shapes, two statuses ───────────────────────

    #[test]
    fn refusals_are_400_and_approvals_are_403() {
        assert_eq!(
            TaskError::Refusal("no".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            TaskError::ApprovalRequired("a person".into()).status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            TaskError::Db(sqlx::Error::RowNotFound).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        // The route's instanceof branch, one place.
        assert_eq!(TaskError::Refusal("no".into()).message(), "no");
    }

    // ── The status-activity sentence ────────────────────────────────────────

    #[test]
    fn status_titles_replace_only_the_first_underscore() {
        // TS String.replace(string, …) replaces the first occurrence — a
        // single replacen, not replace_all, or "needs_info_now" would lose
        // both.
        assert_eq!("needs_info_now".replacen('_', " ", 1), "needs info_now");
        assert_eq!("in_progress".replacen('_', " ", 1), "in progress");
    }
}

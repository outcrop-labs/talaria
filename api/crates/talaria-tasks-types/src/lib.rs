use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use sqlx::PgPool;
use std::sync::{Arc, OnceLock};
use talaria_error::house_error;
use talaria_notify::NotifyDeps;
use talaria_realtime::RealtimeDeps;
use talaria_runs_run::RunDeps;

pub static BUILD_DISPATCH: OnceLock<
    Arc<dyn Fn(PgPool, redis::aio::ConnectionManager, RealtimeDeps) -> RunDeps + Send + Sync>,
> = OnceLock::new();

pub static UPDATE_TASK: OnceLock<
    Arc<
        dyn Fn(TaskDeps, String, TaskPatch, TaskActor) -> BoxFuture<'static, TaskResult<Task>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

#[derive(Clone)]
pub struct TaskDeps {
    pub pg: PgPool,
    pub realtime: RealtimeDeps,
    pub notify: NotifyDeps,
    pub dispatch: Option<RunDeps>,
}

impl TaskDeps {
    pub fn from_route(pg: PgPool, redis: Option<redis::aio::ConnectionManager>) -> Self {
        let realtime = RealtimeDeps::publish_only(redis.clone());
        let notify = NotifyDeps::publishing(pg.clone(), redis.clone());
        let dispatch = redis.and_then(|conn| {
            BUILD_DISPATCH
                .get()
                .map(|f| f(pg.clone(), conn, realtime.clone()))
        });
        TaskDeps {
            pg,
            realtime,
            notify,
            dispatch,
        }
    }
}

pub enum TaskError {
    Refusal(String),
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
    pub fn status(&self) -> StatusCode {
        match self {
            TaskError::Refusal(_) => StatusCode::BAD_REQUEST,
            TaskError::ApprovalRequired(_) => StatusCode::FORBIDDEN,
            TaskError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    pub fn into_response(self) -> axum::response::Response {
        house_error(self.status(), &self.message())
    }
}

pub type TaskResult<T> = Result<T, TaskError>;

pub fn is_human_assignee(a: &str) -> bool {
    a.starts_with("user:")
}
pub fn human_assignee_ids(assignees: &[String]) -> Vec<String> {
    assignees
        .iter()
        .filter(|a| is_human_assignee(a))
        .map(|a| a[5..].to_string())
        .collect()
}
pub fn agent_assignees(assignees: &[String]) -> Vec<String> {
    assignees
        .iter()
        .filter(|a| !is_human_assignee(a))
        .cloned()
        .collect()
}
pub fn json_strings(v: &serde_json::Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub board_id: String,
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

#[derive(Debug, Clone)]
pub struct TaskActor {
    pub kind: TaskActorKind,
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
    pub attachments: Option<serde_json::Value>,
    pub add_time_spent_seconds: Option<f64>,
    pub status_note: Option<String>,
}

pub async fn update_task(
    deps: &TaskDeps,
    id: &str,
    patch: TaskPatch,
    actor: &TaskActor,
) -> TaskResult<Task> {
    match UPDATE_TASK.get() {
        Some(f) => f(deps.clone(), id.to_string(), patch, actor.clone()).await,
        None => Err(TaskError::Refusal("task update is not wired".into())),
    }
}

// The step's edges to the outside, as two traits: `Remote` (ClickUp) and
// `Local` (Talaria). run.rs drives a step against them; the unit tests drive
// it against an in-memory fake (the talaria-runs-rewind world.rs pattern), and
// production against `Real` below — the ClickUp client and the task engine.
//
// THE TASK ENGINE IS THE ONLY WRITER OF TICKETS. Every ticket write here goes
// through talaria-tasks' create_task / update_task with the platform actor,
// so board membership, activity lines, realtime, retrieval indexing and the
// agent invariants all still apply. The sync's writes are QUIET (a mirrored
// write tells nobody; the step publishes one board event per batch).

use std::future::Future;

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_clickup::{
    Account, ClickUpError, Client, Comment, ListInfo, RateLimit, Task, TaskQuery, TasksPage,
};
use talaria_notify::{
    ActorKind, NotificationInput, NotifyActor, NotifyDeps, NotifyMeta, add_notification_with,
    group_key,
};
use talaria_realtime::{BoardEvent, publish_board};
use talaria_tasks::{
    NewTask, TaskActor, TaskDeps, TaskError, TaskPatch, create_task, get_task, update_task,
};

use crate::mapping::{Columns, People, Problem};
use crate::store::{self, Finish, Item, SyncRow, Walk};
use crate::{PROVIDER, SYNC_ACTOR};

/// ClickUp, as the step reads it.
pub trait Remote: Send + Sync {
    fn probe(&self) -> impl Future<Output = Result<Account, ClickUpError>> + Send;
    fn list(&self, list_id: &str) -> impl Future<Output = Result<ListInfo, ClickUpError>> + Send;
    fn tasks_page(
        &self,
        list_id: &str,
        q: &TaskQuery,
    ) -> impl Future<Output = Result<TasksPage, ClickUpError>> + Send;
    fn task(&self, id: &str) -> impl Future<Output = Result<Task, ClickUpError>> + Send;
    fn comments(
        &self,
        task_id: &str,
        before: Option<(i64, String)>,
    ) -> impl Future<Output = Result<Vec<Comment>, ClickUpError>> + Send;
    fn replies(
        &self,
        comment_id: &str,
    ) -> impl Future<Output = Result<Vec<Comment>, ClickUpError>> + Send;
    fn download(
        &self,
        url: &str,
        max: usize,
    ) -> impl Future<Output = Result<Vec<u8>, ClickUpError>> + Send;
    /// The budget as of the last answer.
    fn rate(&self) -> RateLimit;
}

impl Remote for Client {
    async fn probe(&self) -> Result<Account, ClickUpError> {
        self.user().await
    }
    async fn list(&self, list_id: &str) -> Result<ListInfo, ClickUpError> {
        Client::list(self, list_id).await
    }
    async fn tasks_page(&self, list_id: &str, q: &TaskQuery) -> Result<TasksPage, ClickUpError> {
        Client::tasks_page(self, list_id, q).await
    }
    async fn task(&self, id: &str) -> Result<Task, ClickUpError> {
        Client::task(self, id).await
    }
    async fn comments(
        &self,
        task_id: &str,
        before: Option<(i64, String)>,
    ) -> Result<Vec<Comment>, ClickUpError> {
        Client::comments(
            self,
            task_id,
            before.as_ref().map(|(d, id)| (*d, id.as_str())),
        )
        .await
    }
    async fn replies(&self, comment_id: &str) -> Result<Vec<Comment>, ClickUpError> {
        Client::replies(self, comment_id).await
    }
    async fn download(&self, url: &str, max: usize) -> Result<Vec<u8>, ClickUpError> {
        Client::download(self, url, max).await
    }
    fn rate(&self) -> RateLimit {
        Client::rate(self)
    }
}

/// A ticket as the sync reads it before writing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ticket {
    pub id: String,
    pub board_id: String,
    pub ticket_ref: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub due: Option<String>,
    pub start: Option<String>,
    pub tags: Vec<String>,
    pub assignees: Vec<String>,
    pub parent_id: Option<String>,
    pub attachments: Value,
    /// An agent's work session is live on it.
    pub live_session: bool,
}

/// A ticket to make.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewTicket {
    pub title: String,
    pub description: Option<String>,
    pub priority: String,
    pub due: Option<String>,
    pub start: Option<String>,
    pub tags: Vec<String>,
    pub assignees: Vec<String>,
    pub parent_id: Option<String>,
    pub status: String,
    pub external_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Created {
    Made(String),
    /// The unique index on external_key said a ticket already exists — a
    /// crash between its insert and its item row. That ticket is the one.
    Exists(String),
    Refused(String),
}

/// A write to an existing ticket. `None` leaves a field alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketPatch {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<String>,
    pub due: Option<Option<String>>,
    pub start: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub parent_id: Option<Option<String>>,
    pub status: Option<String>,
    pub status_note: Option<String>,
    pub attachments: Option<Value>,
}

impl TicketPatch {
    pub fn is_empty(&self) -> bool {
        *self == TicketPatch::default()
    }
}

/// Talaria, as the step writes it.
pub trait Local: Send + Sync {
    fn now_ms(&self) -> i64;
    fn sync(&self, id: &str) -> impl Future<Output = Result<Option<SyncRow>, String>> + Send;
    fn columns(&self, board_id: &str) -> impl Future<Output = Result<Columns, String>> + Send;
    fn people(&self, board_id: &str) -> impl Future<Output = Result<People, String>> + Send;
    fn item(&self, external_id: &str) -> impl Future<Output = Result<Option<Item>, String>> + Send;
    fn save_item(&self, item: &Item) -> impl Future<Output = Result<(), String>> + Send;
    fn ticket(&self, task_id: &str) -> impl Future<Output = Result<Option<Ticket>, String>> + Send;
    fn create(
        &self,
        board_id: &str,
        t: &NewTicket,
    ) -> impl Future<Output = Result<Created, String>> + Send;
    fn update(
        &self,
        task_id: &str,
        p: &TicketPatch,
    ) -> impl Future<Output = Result<Result<(), String>, String>> + Send;
    /// Land one imported comment in the ticket's room: (message id, whether it
    /// was new). None when the ticket has no room and none can be made.
    fn import_comment(
        &self,
        task_id: &str,
        author: &str,
        text: &str,
        at_ms: Option<i64>,
        thread_root: Option<&str>,
        key: &str,
    ) -> impl Future<Output = Result<Option<(String, bool)>, String>> + Send;
    fn edit_comment(
        &self,
        task_id: &str,
        key: &str,
        text: &str,
    ) -> impl Future<Output = Result<bool, String>> + Send;
    /// Store the bytes as an upload and append it to the ticket's
    /// attachments. The upload id.
    fn attach(
        &self,
        task_id: &str,
        filename: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn activity(
        &self,
        task_id: &str,
        text: &str,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn walk(
        &self,
        board_id: &str,
        walk: Walk,
        run_id: &str,
        after: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Item>, String>> + Send;
    fn mark_seen(
        &self,
        board_id: &str,
        run_id: &str,
        ids: &[String],
    ) -> impl Future<Output = Result<Vec<(String, Option<i64>, bool, String)>, String>> + Send;
    fn unseen_counts(
        &self,
        board_id: &str,
        run_id: &str,
    ) -> impl Future<Output = Result<(i64, i64), String>> + Send;
    fn record_list(
        &self,
        sync: &SyncRow,
        info: &ListInfo,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// The probe refused the token: needs_credential, and one grouped notice
    /// to the board's owners and the admins.
    fn credential_rejected(
        &self,
        sync: &SyncRow,
        problem: &Problem,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// A run-level problem that stops the run (the list is gone): recorded,
    /// and the same notice.
    fn run_stopped(
        &self,
        sync: &SyncRow,
        problem: &Problem,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn finish(&self, sync: &SyncRow, f: &Finish)
    -> impl Future<Output = Result<(), String>> + Send;
    /// One board event for a batch of quiet writes.
    fn publish(&self, board_id: &str);
    /// Apps hear TicketCreated for an imported ticket (only when the
    /// connection says they should).
    fn emit_created(&self, board_id: &str, task_id: &str);
}

// ── The real Talaria ─────────────────────────────────────────────────────────

/// Production `Local`: the pool, the task engine's deps, and the secretbox for
/// uploads.
#[derive(Clone)]
pub struct Real {
    pub pg: PgPool,
    pub tasks: TaskDeps,
    pub sb: talaria_secretbox::SecretBox,
}

fn db(e: sqlx::Error) -> String {
    e.to_string()
}

impl Real {
    /// Board owners (team owners included) and the instance's admins — who
    /// can replace a credential. Ids, de-duplicated.
    async fn sync_stewards(&self, board_id: &str) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "select user_id::text from board_members where board_id = $1::uuid and role = 'owner' \
             union select tm.user_id::text from boards b \
               join team_members tm on tm.team_id = b.team_id and tm.role = 'owner' \
               where b.id = $1::uuid \
             union select id::text from users where role = 'admin'",
        )
        .bind(board_id)
        .fetch_all(&self.pg)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn notify_stewards(&self, sync: &SyncRow, problem: &Problem) -> Result<(), String> {
        let board: Option<(String,)> =
            sqlx::query_as("select name from boards where id = $1::uuid")
                .bind(&sync.board_id)
                .fetch_optional(&self.pg)
                .await
                .map_err(db)?;
        let name = board.map(|(n,)| n).unwrap_or_else(|| "a board".into());
        let title = format!("ClickUp sync stopped on {name}");
        let body = format!("{} {}", problem.detail, problem.fix);
        let href = format!("/boards/{}", sync.board_id);
        let notify = NotifyDeps {
            pg: self.pg.clone(),
            realtime: self.tasks.realtime.clone(),
        };
        for uid in self.sync_stewards(&sync.board_id).await.map_err(db)? {
            let meta = NotifyMeta {
                group_key: Some(group_key::board_sync(&sync.id)),
                actor: Some(NotifyActor {
                    kind: ActorKind::System,
                    id: SYNC_ACTOR.into(),
                    label: "ClickUp sync".into(),
                }),
                ..Default::default()
            };
            if let Err(e) = add_notification_with(
                &notify,
                &uid,
                &NotificationInput {
                    kind: "board-sync",
                    title: &title,
                    body: Some(&body),
                    href: Some(&href),
                },
                &meta,
            )
            .await
            {
                tracing::warn!("[board-sync] notice to {uid} failed: {e}");
            }
        }
        Ok(())
    }
}

impl Local for Real {
    fn now_ms(&self) -> i64 {
        talaria_agent_auth::now_ms()
    }

    async fn sync(&self, id: &str) -> Result<Option<SyncRow>, String> {
        store::sync_by_id(&self.pg, id).await.map_err(db)
    }

    async fn columns(&self, board_id: &str) -> Result<Columns, String> {
        let list = talaria_statuses::list_statuses(&self.pg, board_id)
            .await
            .map_err(db)?;
        Ok(Columns::new(list))
    }

    async fn people(&self, board_id: &str) -> Result<People, String> {
        store::people(&self.pg, board_id).await.map_err(db)
    }

    async fn item(&self, external_id: &str) -> Result<Option<Item>, String> {
        store::item(&self.pg, PROVIDER, external_id)
            .await
            .map_err(db)
    }

    async fn save_item(&self, item: &Item) -> Result<(), String> {
        store::save_item(&self.pg, PROVIDER, item).await.map_err(db)
    }

    async fn ticket(&self, task_id: &str) -> Result<Option<Ticket>, String> {
        let Some(t) = get_task(&self.pg, task_id).await.map_err(db)? else {
            return Ok(None);
        };
        let live: Option<(i32,)> = sqlx::query_as(
            "select 1 from runs where kind = 'work-session' and subject_id = $1 \
               and state in ('queued', 'running', 'awaiting') limit 1",
        )
        .bind(task_id)
        .fetch_optional(&self.pg)
        .await
        .map_err(db)?;
        Ok(Some(Ticket {
            id: t.id,
            board_id: t.board_id,
            ticket_ref: t.ticket_ref,
            title: t.title,
            description: t.description,
            status: t.status,
            priority: t.priority,
            due: t.due_date,
            start: t.start_date,
            tags: t.tags,
            assignees: t.assignees,
            parent_id: t.parent_id,
            attachments: t.attachments,
            live_session: live.is_some(),
        }))
    }

    async fn create(&self, board_id: &str, t: &NewTicket) -> Result<Created, String> {
        let input = NewTask {
            board_id,
            title: &t.title,
            description: t.description.as_deref(),
            priority: Some(&t.priority),
            effort: None,
            assignees: &t.assignees,
            due_date: t.due.as_deref(),
            start_date: t.start.as_deref(),
            color: None,
            estimated_hours: None,
            parent_id: t.parent_id.as_deref(),
            tags: &t.tags,
            created_by: SYNC_ACTOR,
            status: Some(&t.status),
            external_key: Some(&t.external_key),
            quiet: true,
        };
        match create_task(&self.tasks, &input).await {
            Ok(task) => Ok(Created::Made(task.id)),
            Err(TaskError::Db(e))
                if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505") =>
            {
                let row: Option<(String,)> =
                    sqlx::query_as("select id::text from tasks where external_key = $1")
                        .bind(&t.external_key)
                        .fetch_optional(&self.pg)
                        .await
                        .map_err(db)?;
                match row {
                    Some((id,)) => Ok(Created::Exists(id)),
                    None => Err(e.to_string()),
                }
            }
            Err(TaskError::Db(e)) => Err(e.to_string()),
            Err(e) => Ok(Created::Refused(e.message())),
        }
    }

    async fn update(&self, task_id: &str, p: &TicketPatch) -> Result<Result<(), String>, String> {
        let patch = TaskPatch {
            title: p.title.clone(),
            description: p.description.clone(),
            status: p.status.clone(),
            priority: p.priority.clone(),
            assignees: p.assignees.clone(),
            due_date: p.due.clone(),
            start_date: p.start.clone(),
            tags: p.tags.clone(),
            parent_id: p.parent_id.clone(),
            attachments: p.attachments.clone(),
            status_note: p.status_note.clone(),
            quiet: true,
            ..Default::default()
        };
        match update_task(
            &self.tasks,
            task_id,
            patch,
            &TaskActor::platform(SYNC_ACTOR),
        )
        .await
        {
            Ok(Some(_)) => Ok(Ok(())),
            Ok(None) => Ok(Err("the ticket no longer exists".into())),
            Err(TaskError::Db(e)) => Err(e.to_string()),
            Err(e) => Ok(Err(e.message())),
        }
    }

    async fn import_comment(
        &self,
        task_id: &str,
        author: &str,
        text: &str,
        at_ms: Option<i64>,
        thread_root: Option<&str>,
        key: &str,
    ) -> Result<Option<(String, bool)>, String> {
        let Some(room) = talaria_tasks::ensure_task_channel(&self.pg, task_id)
            .await
            .map_err(db)?
        else {
            return Ok(None);
        };
        let notify = NotifyDeps {
            pg: self.pg.clone(),
            realtime: self.tasks.realtime.clone(),
        };
        let landed = talaria_channels::insert_imported_message(
            &notify,
            &room,
            author,
            text,
            at_ms,
            thread_root,
            key,
        )
        .await
        .map_err(db)?;
        Ok(Some((landed.id, landed.inserted)))
    }

    async fn edit_comment(&self, task_id: &str, key: &str, text: &str) -> Result<bool, String> {
        let Some(room) = talaria_tasks::task_room_channel_id(&self.pg, task_id)
            .await
            .map_err(db)?
        else {
            return Ok(false);
        };
        let notify = NotifyDeps {
            pg: self.pg.clone(),
            realtime: self.tasks.realtime.clone(),
        };
        talaria_channels::update_imported_message(&notify, &room, key, text)
            .await
            .map_err(db)
    }

    async fn attach(
        &self,
        task_id: &str,
        filename: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<String, String> {
        let up =
            talaria_uploads::save_upload(&self.pg, &self.sb, filename, mime, &bytes, None).await?;
        // Read-append-write through the engine. One live sync per board means
        // the sync never races itself here; a person attaching a file in the
        // same instant is the window, and their file stays on its upload row.
        let Some(t) = get_task(&self.pg, task_id).await.map_err(db)? else {
            return Err("the ticket no longer exists".into());
        };
        let mut list = t.attachments.as_array().cloned().unwrap_or_default();
        list.push(json!({"id": up.id, "filename": up.filename, "mime": up.mime, "size": up.size}));
        let patch = TicketPatch {
            attachments: Some(Value::Array(list)),
            ..Default::default()
        };
        match self.update(task_id, &patch).await? {
            Ok(()) => Ok(up.id),
            Err(why) => Err(why),
        }
    }

    async fn activity(&self, task_id: &str, text: &str) -> Result<(), String> {
        talaria_tasks::log_activity(&self.pg, task_id, SYNC_ACTOR, "comment", text)
            .await
            .map_err(db)
    }

    async fn walk(
        &self,
        board_id: &str,
        walk: Walk,
        run_id: &str,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Item>, String> {
        store::walk_items(&self.pg, board_id, walk, run_id, after, limit)
            .await
            .map_err(db)
    }

    async fn mark_seen(
        &self,
        board_id: &str,
        run_id: &str,
        ids: &[String],
    ) -> Result<Vec<(String, Option<i64>, bool, String)>, String> {
        store::mark_seen(&self.pg, PROVIDER, board_id, run_id, ids)
            .await
            .map_err(db)
    }

    async fn unseen_counts(&self, board_id: &str, run_id: &str) -> Result<(i64, i64), String> {
        store::unseen_counts(&self.pg, board_id, run_id)
            .await
            .map_err(db)
    }

    async fn record_list(&self, sync: &SyncRow, info: &ListInfo) -> Result<(), String> {
        let statuses = serde_json::to_value(&info.statuses).unwrap_or_else(|_| json!([]));
        store::record_list(&self.pg, &sync.id, &info.path(), &statuses, None)
            .await
            .map_err(db)
    }

    async fn credential_rejected(&self, sync: &SyncRow, problem: &Problem) -> Result<(), String> {
        // Only the transition files the notice: a second refused run on a
        // sync already waiting for a credential tells nobody again.
        if store::set_needs_credential(&self.pg, &sync.id, problem)
            .await
            .map_err(db)?
        {
            self.notify_stewards(sync, problem).await?;
        }
        self.publish(&sync.board_id);
        Ok(())
    }

    async fn run_stopped(&self, sync: &SyncRow, problem: &Problem) -> Result<(), String> {
        store::record_run_problems(&self.pg, &sync.id, std::slice::from_ref(problem))
            .await
            .map_err(db)?;
        self.notify_stewards(sync, problem).await?;
        self.publish(&sync.board_id);
        Ok(())
    }

    async fn finish(&self, sync: &SyncRow, f: &Finish) -> Result<(), String> {
        store::finish(&self.pg, &sync.id, f).await.map_err(db)
    }

    fn publish(&self, board_id: &str) {
        publish_board(
            &self.tasks.realtime,
            board_id,
            &BoardEvent {
                kind_tag: "board",
                task_id: None,
                deleted: None,
            },
        );
    }

    fn emit_created(&self, board_id: &str, task_id: &str) {
        talaria_platform_events::emit(talaria_platform_events::PlatformEvent::TicketCreated {
            ticket_id: task_id.to_string(),
            board_id: board_id.to_string(),
        });
    }
}

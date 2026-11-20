// The run's state machine against an in-memory world: a fake ClickUp and a
// fake Talaria, a clock that moves on every read. What is under test is the
// CHECKPOINT discipline — what a step writes, where a re-entry lands, what a
// back-off saves — not SQL (the live suite, api/tests/it/board_sync_live.rs,
// drives the real engine against a loopback ClickUp stand-in).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use talaria_clickup::{
    Account, ClickUpError, Comment, ListInfo, RateLimit, Task, TaskQuery, TasksPage,
};
use talaria_runs_define::{StepResult, StepSignal};
use talaria_statuses::BoardStatus;

use crate::mapping::{Columns, People, Problem, StatusMap};
use crate::run::{Checkpoint, Pass, run_step};
use crate::store::{Finish, Item, SyncOptions, SyncRow, Walk};
use crate::world::{Created, Local, NewTicket, Remote, Ticket, TicketPatch};

// ── the fake ClickUp ─────────────────────────────────────────────────────────

#[derive(Default)]
struct RemoteState {
    tasks: Vec<Task>,
    archived: Vec<Task>,
    comments: HashMap<String, Vec<Comment>>, // newest first, as ClickUp pages them
    replies: HashMap<String, Vec<Comment>>,
    probe_refused: bool,
    page_size: usize,
    /// After this many calls, every call answers 429 until cleared.
    limit_after: Option<usize>,
    calls: usize,
    remaining: Option<i64>,
    /// What GET /task answers, when it differs from the list page.
    details: HashMap<String, Task>,
    downloads: Vec<String>,
}

#[derive(Clone, Default)]
struct FakeRemote(Arc<Mutex<RemoteState>>);

impl FakeRemote {
    fn new(tasks: Vec<Task>) -> FakeRemote {
        let r = FakeRemote::default();
        {
            let mut s = r.0.lock().unwrap();
            s.tasks = tasks;
            s.page_size = 100;
        }
        r
    }
    fn gate(&self) -> Result<(), ClickUpError> {
        let mut s = self.0.lock().unwrap();
        s.calls += 1;
        if let Some(n) = s.limit_after
            && s.calls > n
        {
            return Err(ClickUpError::RateLimited {
                reset_ms: Some(10_000_000),
            });
        }
        Ok(())
    }
    fn calls(&self) -> usize {
        self.0.lock().unwrap().calls
    }
}

impl Remote for FakeRemote {
    async fn probe(&self) -> Result<Account, ClickUpError> {
        self.gate()?;
        if self.0.lock().unwrap().probe_refused {
            return Err(ClickUpError::Unauthorized {
                ecode: Some("OAUTH_025".into()),
            });
        }
        Ok(Account {
            id: "1".into(),
            username: "sync".into(),
            email: None,
        })
    }
    async fn list(&self, list_id: &str) -> Result<ListInfo, ClickUpError> {
        self.gate()?;
        Ok(ListInfo {
            id: list_id.into(),
            name: "List".into(),
            space: None,
            folder: None,
            statuses: vec![],
            task_count: None,
        })
    }
    async fn tasks_page(&self, _list_id: &str, q: &TaskQuery) -> Result<TasksPage, ClickUpError> {
        self.gate()?;
        let s = self.0.lock().unwrap();
        let all: Vec<Task> = if q.archived {
            s.archived.clone()
        } else {
            s.tasks.clone()
        };
        let all: Vec<Task> = all
            .into_iter()
            .filter(|t| {
                q.updated_after_ms
                    .is_none_or(|a| t.updated_ms.unwrap_or(0) > a)
            })
            .collect();
        let from = q.page as usize * s.page_size;
        let tasks: Vec<Task> = all.iter().skip(from).take(s.page_size).cloned().collect();
        Ok(TasksPage {
            last_page: from + s.page_size >= all.len(),
            tasks,
        })
    }
    async fn task(&self, id: &str) -> Result<Task, ClickUpError> {
        self.gate()?;
        let s = self.0.lock().unwrap();
        if let Some(t) = s.details.get(id) {
            return Ok(t.clone());
        }
        s.tasks
            .iter()
            .chain(s.archived.iter())
            .find(|t| t.id == id)
            .cloned()
            .map(|mut t| {
                t.attachments.get_or_insert_with(Vec::new);
                t
            })
            .ok_or(ClickUpError::NotFound)
    }
    async fn comments(
        &self,
        task_id: &str,
        before: Option<(i64, String)>,
    ) -> Result<Vec<Comment>, ClickUpError> {
        self.gate()?;
        let s = self.0.lock().unwrap();
        let all = s.comments.get(task_id).cloned().unwrap_or_default();
        let start = match before {
            Some((_, id)) => all
                .iter()
                .position(|c| c.id == id)
                .map(|i| i + 1)
                .unwrap_or(all.len()),
            None => 0,
        };
        Ok(all
            .into_iter()
            .skip(start)
            .take(talaria_clickup::COMMENT_PAGE)
            .collect())
    }
    async fn replies(&self, comment_id: &str) -> Result<Vec<Comment>, ClickUpError> {
        self.gate()?;
        Ok(self
            .0
            .lock()
            .unwrap()
            .replies
            .get(comment_id)
            .cloned()
            .unwrap_or_default())
    }
    async fn download(&self, url: &str, _max: usize) -> Result<Vec<u8>, ClickUpError> {
        self.gate()?;
        self.0.lock().unwrap().downloads.push(url.to_string());
        Ok(b"bytes".to_vec())
    }
    fn rate(&self) -> RateLimit {
        RateLimit {
            limit: Some(100),
            remaining: self.0.lock().unwrap().remaining,
            reset_ms: Some(10_000_000),
        }
    }
}

// ── the fake Talaria ─────────────────────────────────────────────────────────

#[derive(Default)]
struct LocalState {
    sync: Option<SyncRow>,
    cols: Vec<BoardStatus>,
    people: People,
    items: BTreeMap<String, Item>,
    tickets: BTreeMap<String, Ticket>,
    keys: HashMap<String, String>, // external_key → ticket id
    next: u32,
    /// (task id, import key, author, text, at, thread root)
    comments: Vec<(String, String, String, String, Option<i64>, Option<String>)>,
    edits: usize,
    finishes: Vec<Finish>,
    rejected: usize,
    seen: HashMap<String, String>,
    /// The next status write dies with the process (a crash between the
    /// create and the move): the step errors, nothing after it lands.
    crash_next_move: bool,
    creates: usize,
}

#[derive(Clone)]
struct FakeLocal {
    st: Arc<Mutex<LocalState>>,
    clock: Arc<AtomicI64>,
    tick: Arc<AtomicI64>,
    reads: Arc<AtomicUsize>,
    /// The run id the next steps use; `drive` starts a new run each call.
    run: Arc<AtomicUsize>,
}

fn status(key: &str, category: &str, agent_start: bool) -> BoardStatus {
    BoardStatus {
        id: Some(key.into()),
        key: key.into(),
        label: key.replace('_', " "),
        color: "slate".into(),
        category: category.into(),
        agent_start,
        position: 0,
        system: None,
    }
}

fn sync_row(map: &[(&str, &str)]) -> SyncRow {
    SyncRow {
        id: "sync-1".into(),
        board_id: "board-1".into(),
        provider: "clickup".into(),
        workspace_id: None,
        list_id: "L".into(),
        list_path: None,
        list_statuses: json!([]),
        credential_sealed: Some("sealed".into()),
        credential_hint: None,
        credential_account: None,
        status_map: map
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        map_version: 1,
        remap_pending: false,
        options: SyncOptions::default(),
        interval_minutes: Some(60),
        state: "active".into(),
        watermark_ms: None,
        last_sweep_ms: None,
        last_synced_ms: None,
        next_sync_ms: None,
        last_run_id: None,
        run_problems: vec![],
        connected_by: None,
        created_ms: 0,
        updated_ms: 0,
    }
}

impl FakeLocal {
    fn new(map: &[(&str, &str)]) -> FakeLocal {
        let st = LocalState {
            sync: Some(sync_row(map)),
            cols: vec![
                status("inbox", "open", false),
                status("assigned", "open", true),
                status("in_progress", "active", true),
                status("doing", "active", false),
                status("blocked", "blocked", false),
                status("quality_review", "review", false),
                status("done", "done", false),
            ],
            ..Default::default()
        };
        FakeLocal {
            st: Arc::new(Mutex::new(st)),
            clock: Arc::new(AtomicI64::new(1_000_000)),
            tick: Arc::new(AtomicI64::new(1)),
            reads: Arc::new(AtomicUsize::new(0)),
            run: Arc::new(AtomicUsize::new(1)),
        }
    }
    fn with<T>(&self, f: impl FnOnce(&mut LocalState) -> T) -> T {
        f(&mut self.st.lock().unwrap())
    }
    fn ticket_of(&self, ext: &str) -> Option<Ticket> {
        self.with(|s| {
            let id = s.items.get(ext)?.task_id.clone()?;
            s.tickets.get(&id).cloned()
        })
    }
    fn item_of(&self, ext: &str) -> Option<Item> {
        self.with(|s| s.items.get(ext).cloned())
    }
}

impl Local for FakeLocal {
    fn now_ms(&self) -> i64 {
        self.clock
            .fetch_add(self.tick.load(Ordering::Relaxed), Ordering::Relaxed)
    }
    async fn sync(&self, _id: &str) -> Result<Option<SyncRow>, String> {
        Ok(self.with(|s| s.sync.clone()))
    }
    async fn columns(&self, _board_id: &str) -> Result<Columns, String> {
        Ok(Columns::new(self.with(|s| s.cols.clone())))
    }
    async fn people(&self, _board_id: &str) -> Result<People, String> {
        Ok(self.with(|s| s.people.clone()))
    }
    async fn item(&self, external_id: &str) -> Result<Option<Item>, String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(self.with(|s| s.items.get(external_id).cloned()))
    }
    async fn save_item(&self, item: &Item) -> Result<(), String> {
        self.with(|s| s.items.insert(item.external_id.clone(), item.clone()));
        Ok(())
    }
    async fn ticket(&self, task_id: &str) -> Result<Option<Ticket>, String> {
        Ok(self.with(|s| s.tickets.get(task_id).cloned()))
    }
    async fn create(&self, board_id: &str, t: &NewTicket) -> Result<Created, String> {
        Ok(self.with(|s| {
            if let Some(id) = s.keys.get(&t.external_key) {
                return Created::Exists(id.clone());
            }
            s.next += 1;
            s.creates += 1;
            let id = format!("task-{}", s.next);
            s.keys.insert(t.external_key.clone(), id.clone());
            s.tickets.insert(
                id.clone(),
                Ticket {
                    id: id.clone(),
                    board_id: board_id.into(),
                    ticket_ref: Some(format!("T-{}", s.next)),
                    title: t.title.clone(),
                    description: t.description.clone(),
                    status: t.status.clone(),
                    priority: t.priority.clone(),
                    due: t.due.clone(),
                    start: t.start.clone(),
                    tags: t.tags.clone(),
                    assignees: t.assignees.clone(),
                    parent_id: t.parent_id.clone(),
                    attachments: json!([]),
                    live_session: false,
                },
            );
            Created::Made(id)
        }))
    }
    async fn update(&self, task_id: &str, p: &TicketPatch) -> Result<Result<(), String>, String> {
        let crash = self.with(|s| {
            let c = p.status.is_some() && s.crash_next_move;
            if c {
                s.crash_next_move = false;
            }
            c
        });
        if crash {
            return Err("simulated crash".into());
        }
        Ok(self.with(|s| {
            let Some(t) = s.tickets.get_mut(task_id) else {
                return Err("gone".into());
            };
            if let Some(v) = &p.title {
                t.title = v.clone();
            }
            if let Some(v) = &p.description {
                t.description = v.clone();
            }
            if let Some(v) = &p.priority {
                t.priority = v.clone();
            }
            if let Some(v) = &p.tags {
                t.tags = v.clone();
            }
            if let Some(v) = &p.assignees {
                t.assignees = v.clone();
            }
            if let Some(v) = &p.status {
                t.status = v.clone();
            }
            if let Some(v) = &p.parent_id {
                t.parent_id = v.clone();
            }
            if let Some(v) = &p.attachments {
                t.attachments = v.clone();
            }
            Ok(())
        }))
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
        Ok(self.with(|s| {
            if let Some(i) = s.comments.iter().position(|c| c.1 == key) {
                return Some((format!("m{i}"), false));
            }
            s.comments.push((
                task_id.into(),
                key.into(),
                author.into(),
                text.into(),
                at_ms,
                thread_root.map(String::from),
            ));
            Some((format!("m{}", s.comments.len() - 1), true))
        }))
    }
    async fn edit_comment(&self, _task_id: &str, key: &str, text: &str) -> Result<bool, String> {
        Ok(self.with(|s| {
            if let Some(c) = s.comments.iter_mut().find(|c| c.1 == key) {
                c.3 = text.into();
                s.edits += 1;
                return true;
            }
            false
        }))
    }
    async fn attach(
        &self,
        task_id: &str,
        filename: &str,
        _mime: &str,
        _bytes: Vec<u8>,
    ) -> Result<String, String> {
        self.with(|s| {
            if let Some(t) = s.tickets.get_mut(task_id) {
                t.attachments
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"filename": filename}));
            }
        });
        Ok(format!("up-{filename}"))
    }
    async fn activity(&self, _task_id: &str, _text: &str) -> Result<(), String> {
        Ok(())
    }
    async fn walk(
        &self,
        board_id: &str,
        walk: Walk,
        run_id: &str,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Item>, String> {
        Ok(self.with(|s| {
            s.items
                .values()
                .filter(|i| i.board_id == board_id && i.task_id.is_some())
                .filter(|i| after.is_none_or(|a| i.external_id.as_str() > a))
                .filter(|i| match walk {
                    Walk::Linked => true,
                    Walk::Unseen => {
                        i.remote_state == "active"
                            && s.seen.get(&i.external_id).map(String::as_str) != Some(run_id)
                    }
                    Walk::PendingAttachments => i.has_pending_attachments(),
                    Walk::PendingParents => i.applied.parent_pending.is_some(),
                })
                .take(limit as usize)
                .cloned()
                .collect()
        }))
    }
    async fn mark_seen(
        &self,
        board_id: &str,
        run_id: &str,
        ids: &[String],
    ) -> Result<Vec<(String, Option<i64>, bool, String)>, String> {
        Ok(self.with(|s| {
            let mut out = Vec::new();
            for id in ids {
                if let Some(i) = s.items.get(id).cloned() {
                    if i.board_id == board_id {
                        s.seen.insert(id.clone(), run_id.into());
                    }
                    out.push((
                        id.clone(),
                        i.remote_updated_ms,
                        i.task_id.is_none(),
                        i.board_id.clone(),
                    ));
                }
            }
            out
        }))
    }
    async fn unseen_counts(&self, board_id: &str, run_id: &str) -> Result<(i64, i64), String> {
        Ok(self.with(|s| {
            let linked: Vec<&Item> = s
                .items
                .values()
                .filter(|i| {
                    i.board_id == board_id && i.task_id.is_some() && i.remote_state == "active"
                })
                .collect();
            let unseen = linked
                .iter()
                .filter(|i| s.seen.get(&i.external_id).map(String::as_str) != Some(run_id))
                .count();
            (linked.len() as i64, unseen as i64)
        }))
    }
    async fn record_list(&self, _sync: &SyncRow, _info: &ListInfo) -> Result<(), String> {
        Ok(())
    }
    async fn credential_rejected(&self, _sync: &SyncRow, _problem: &Problem) -> Result<(), String> {
        self.with(|s| {
            s.rejected += 1;
            if let Some(sync) = s.sync.as_mut() {
                sync.state = "needs_credential".into();
            }
        });
        Ok(())
    }
    async fn run_stopped(&self, _sync: &SyncRow, _problem: &Problem) -> Result<(), String> {
        Ok(())
    }
    async fn finish(&self, _sync: &SyncRow, f: &Finish) -> Result<(), String> {
        self.with(|s| {
            s.finishes.push(f.clone());
            if let Some(sync) = s.sync.as_mut() {
                if let Some(w) = f.watermark_ms {
                    sync.watermark_ms = Some(w);
                }
                if f.swept {
                    sync.last_sweep_ms = Some(0);
                }
                if f.remap_version == Some(sync.map_version) {
                    sync.remap_pending = false;
                }
            }
        });
        Ok(())
    }
    fn publish(&self, _board_id: &str) {}
    fn emit_created(&self, _board_id: &str, _task_id: &str) {}
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn task(id: &str, status: &str, updated: i64) -> Task {
    Task::parse(&json!({
        "id": id,
        "name": format!("Task {id}"),
        "status": {"status": status, "type": if status == "complete" { "closed" } else { "custom" }},
        "date_updated": updated.to_string(),
        "date_created": "1",
    }))
    .unwrap()
}

fn comment(id: &str, text: &str, at: i64, replies: i64) -> Comment {
    Comment {
        id: id.into(),
        text: text.into(),
        user: Some(talaria_clickup::Person {
            id: None,
            username: Some("Dana Reyes".into()),
            email: Some("dana@example.com".into()),
        }),
        date_ms: Some(at),
        reply_count: replies,
    }
}

fn input(mode: &str) -> Value {
    json!({"syncId": "sync-1", "mode": mode})
}

/// One step of the current run, its Err included.
async fn try_step(
    local: &FakeLocal,
    remote: &FakeRemote,
    input: &Value,
    cp: &Value,
) -> Result<StepResult, String> {
    let (_tx, signal) = StepSignal::channel();
    let r = remote.clone();
    let run_id = format!("run-{}", local.run.load(Ordering::Relaxed));
    run_step(&run_id, input, cp, &signal, local, move |_s: &SyncRow| {
        Ok(r.clone())
    })
    .await
}

async fn step(local: &FakeLocal, remote: &FakeRemote, input: &Value, cp: &Value) -> StepResult {
    try_step(local, remote, input, cp).await.unwrap()
}

/// Steps until Done (a Retry advances the fake clock past its wait). The
/// results, and the checkpoint the last Next saved.
async fn drive(local: &FakeLocal, remote: &FakeRemote, input: &Value) -> (Vec<StepResult>, Value) {
    local.run.fetch_add(1, Ordering::Relaxed);
    resume(local, remote, input, Value::Null).await
}

/// Steps the current run from `cp` until Done.
async fn resume(
    local: &FakeLocal,
    remote: &FakeRemote,
    input: &Value,
    cp: Value,
) -> (Vec<StepResult>, Value) {
    let mut cp = cp;
    let mut out = Vec::new();
    for _ in 0..500 {
        let r = step(local, remote, input, &cp).await;
        match &r {
            StepResult::Next { checkpoint, .. } => cp = checkpoint.clone(),
            StepResult::Retry { after, .. } => {
                local
                    .clock
                    .fetch_add(after.as_millis() as i64 + 1, Ordering::Relaxed);
            }
            StepResult::Done { .. } => {
                out.push(r);
                return (out, cp);
            }
            StepResult::Decide { .. } => panic!("the sync never asks"),
        }
        out.push(r);
    }
    panic!("the run never finished");
}

fn cp_of(r: &StepResult) -> Checkpoint {
    match r {
        StepResult::Next { checkpoint, .. } => serde_json::from_value(checkpoint.clone()).unwrap(),
        other => panic!("expected Next, got {other:?}"),
    }
}

// ── the tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_first_sync_creates_tickets_places_them_and_advances_the_watermark_only_at_the_end() {
    let local = FakeLocal::new(&[("in progress", "doing"), ("complete", "done")]);
    local.with(|s| s.sync.as_mut().unwrap().options.include_closed = true);
    let remote = FakeRemote::new(vec![
        task("a", "in progress", 10),
        task("b", "complete", 11),
        task("c", "qa", 12),
    ]);
    let (results, _) = drive(&local, &remote, &input("manual")).await;
    // Every Next before Done left the watermark alone.
    assert_eq!(
        local.with(|s| s.finishes.len()),
        1,
        "finish runs once, at the end"
    );
    assert!(local.with(|s| s.finishes[0].watermark_ms.is_some()));
    assert!(matches!(results.last(), Some(StepResult::Done { .. })));
    assert_eq!(local.ticket_of("a").unwrap().status, "doing");
    // A closed task is made in the intake and then moved: two writes.
    assert_eq!(local.ticket_of("b").unwrap().status, "done");
    assert_eq!(
        local.item_of("b").unwrap().applied.status.as_deref(),
        Some("complete")
    );
    // Unmapped: parked in the intake, with a problem, never guessed.
    let c = local.item_of("c").unwrap();
    assert_eq!(local.ticket_of("c").unwrap().status, "inbox");
    assert!(c.problems.iter().any(|p| p.kind == "unmapped-status"));
}

#[tokio::test]
async fn re_entering_the_same_checkpoint_makes_no_second_ticket_and_no_second_comment() {
    let local = FakeLocal::new(&[("in progress", "doing")]);
    let remote = FakeRemote::new(vec![task("a", "in progress", 10)]);
    remote.0.lock().unwrap().comments.insert(
        "a".into(),
        vec![
            comment("c2", "second", 20, 0),
            comment("c1", "first", 10, 0),
        ],
    );
    // The first step makes the ticket and lands its comments...
    let _ = step(&local, &remote, &input("manual"), &Value::Null).await;
    // ...and "did not checkpoint": the item row's last write is lost too, as
    // if the process died right after the comments landed. The reclaim
    // re-enters with the LAST PERSISTED checkpoint — the null one.
    local.with(|s| {
        let it = s.items.get_mut("a").unwrap();
        it.comments.clear();
        it.remote_updated_ms = None;
    });
    let _ = step(&local, &remote, &input("manual"), &Value::Null).await;
    assert_eq!(
        local.with(|s| s.creates),
        1,
        "one ticket, whatever re-entered"
    );
    let comments = local.with(|s| s.comments.clone());
    assert_eq!(comments.len(), 2, "one row per ClickUp comment");
    // Oldest first: the room's order is the history's.
    assert_eq!(comments[0].3, "first");
    assert_eq!(comments[1].3, "second");
    assert_eq!(comments[0].2, "Dana Reyes (ClickUp)");
    assert_eq!(comments[0].4, Some(10), "the original time");
    assert_eq!(
        local.item_of("a").unwrap().comments.len(),
        2,
        "the re-entry re-learned both"
    );
}

#[tokio::test]
async fn a_refused_credential_stops_the_run_before_any_pass() {
    let local = FakeLocal::new(&[]);
    let remote = FakeRemote::new(vec![task("a", "x", 1)]);
    remote.0.lock().unwrap().probe_refused = true;
    let (_tx, signal) = StepSignal::channel();
    let r = remote.clone();
    let out = run_step(
        "run-1",
        &input("scheduled"),
        &Value::Null,
        &signal,
        &local,
        move |_s: &SyncRow| Ok(r.clone()),
    )
    .await;
    assert!(out.is_err());
    assert_eq!(local.with(|s| s.rejected), 1);
    assert_eq!(
        local.with(|s| s.sync.as_ref().unwrap().state.clone()),
        "needs_credential"
    );
    assert_eq!(
        local.with(|s| s.creates),
        0,
        "no task was read off a dead token"
    );
    assert_eq!(remote.calls(), 1, "only the probe was sent");
}

#[tokio::test]
async fn the_rate_limit_saves_the_position_then_retries_after_the_reset() {
    let local = FakeLocal::new(&[("x", "doing")]);
    local.with(|s| {
        let o = &mut s.sync.as_mut().unwrap().options;
        o.comments = false;
        o.attachments = false;
    });
    let remote = FakeRemote::new(
        (0..5)
            .map(|i| task(&format!("t{i}"), "x", 10 + i))
            .collect(),
    );
    // ClickUp says 3 requests remain, under the floor: the first unit of the
    // walk backs off, after the probe.
    remote.0.lock().unwrap().remaining = Some(3);
    let r = step(&local, &remote, &input("manual"), &Value::Null).await;
    let cp = cp_of(&r);
    assert!(
        cp.wait_until_ms.is_some(),
        "the back-off is saved in the checkpoint"
    );
    assert_eq!(cp.pass, Pass::Incremental, "the probe's progress is kept");
    if let StepResult::Next { phase, .. } = &r {
        assert!(phase.as_deref().unwrap_or("").contains("rate limit"));
    }
    // The next step answers Retry with the wait, and saves nothing.
    let cp_v = serde_json::to_value(&cp).unwrap();
    match step(&local, &remote, &input("manual"), &cp_v).await {
        StepResult::Retry { after, reason } => {
            assert!(after.as_millis() > 0);
            assert!(reason.contains("rate limit"));
        }
        other => panic!("expected Retry, got {other:?}"),
    }
    // Past the reset, with budget again, the run resumes where it stopped.
    remote.0.lock().unwrap().remaining = None;
    local.clock.fetch_add(20_000_000, Ordering::Relaxed);
    resume(&local, &remote, &input("manual"), cp_v).await;
    assert_eq!(local.with(|s| s.creates), 5);
}

#[tokio::test]
async fn a_429_inside_a_task_keeps_its_index_and_the_re_entry_finishes_it() {
    let local = FakeLocal::new(&[("x", "doing")]);
    let remote = FakeRemote::new(vec![task("a", "x", 10), task("b", "x", 11)]);
    remote
        .0
        .lock()
        .unwrap()
        .comments
        .insert("b".into(), vec![comment("cb", "hi", 5, 0)]);
    // probe (1), list (2), page (3), a's comments (4), a's detail (5), then
    // b's comments answer 429.
    remote.0.lock().unwrap().limit_after = Some(5);
    let r = step(&local, &remote, &input("manual"), &Value::Null).await;
    let cp = cp_of(&r);
    assert_eq!(cp.index, 1, "a landed; b is where the re-entry starts");
    assert!(cp.wait_until_ms.is_some());
    remote.0.lock().unwrap().limit_after = None;
    local.clock.fetch_add(20_000_000, Ordering::Relaxed);
    resume(
        &local,
        &remote,
        &input("manual"),
        serde_json::to_value(&cp).unwrap(),
    )
    .await;
    assert_eq!(local.with(|s| s.creates), 2, "b's ticket was made once");
    assert_eq!(local.with(|s| s.comments.len()), 1);
}

#[tokio::test]
async fn the_time_budget_checkpoints_mid_page() {
    let local = FakeLocal::new(&[("x", "doing")]);
    local.with(|s| s.sync.as_mut().unwrap().options.comments = false);
    local.with(|s| s.sync.as_mut().unwrap().options.attachments = false);
    let remote = FakeRemote::new(
        (0..30)
            .map(|i| task(&format!("t{i:02}"), "x", 10 + i))
            .collect(),
    );
    // Every clock read costs ten seconds: the 60 s budget runs out a few
    // tasks in.
    local.tick.store(10_000, Ordering::Relaxed);
    let r = step(&local, &remote, &input("manual"), &Value::Null).await;
    let cp = cp_of(&r);
    assert_eq!(cp.pass, Pass::Incremental);
    assert!(
        cp.index > 0 && cp.index < 30,
        "stopped part-way: {}",
        cp.index
    );
    assert_eq!(local.with(|s| s.creates), cp.index);
    assert!(
        cp.wait_until_ms.is_none(),
        "a time stop is a plain Next, not a wait"
    );
}

#[tokio::test]
async fn a_crash_between_create_and_move_leaves_the_status_unapplied_and_the_re_entry_moves_it() {
    let local = FakeLocal::new(&[("complete", "done")]);
    local.with(|s| {
        let o = &mut s.sync.as_mut().unwrap().options;
        o.include_closed = true;
        o.comments = false;
        o.attachments = false;
        s.crash_next_move = true;
    });
    let remote = FakeRemote::new(vec![task("a", "complete", 10)]);
    local.run.fetch_add(1, Ordering::Relaxed);
    assert!(
        try_step(&local, &remote, &input("manual"), &Value::Null)
            .await
            .is_err()
    );
    assert_eq!(
        local.ticket_of("a").unwrap().status,
        "inbox",
        "made in the intake; the move died"
    );
    assert_eq!(
        local.item_of("a").unwrap().applied.status,
        None,
        "applied records only what landed — never 'complete' before the move"
    );
    // The reclaim re-enters with the last persisted checkpoint.
    resume(&local, &remote, &input("manual"), Value::Null).await;
    assert_eq!(local.ticket_of("a").unwrap().status, "done");
    assert_eq!(
        local.item_of("a").unwrap().applied.status.as_deref(),
        Some("complete")
    );
    assert_eq!(local.with(|s| s.creates), 1);
}

#[tokio::test]
async fn the_sync_never_moves_a_ticket_out_of_review_or_blocked_or_into_pickup_with_agents() {
    let local = FakeLocal::new(&[("x", "doing"), ("y", "done"), ("z", "assigned")]);
    local.with(|s| {
        let o = &mut s.sync.as_mut().unwrap().options;
        o.comments = false;
        o.attachments = false;
    });
    let remote = FakeRemote::new(vec![
        task("r", "x", 10),
        task("b", "x", 10),
        task("p", "x", 10),
    ]);
    drive(&local, &remote, &input("manual")).await;
    // A person moves r into review, b into Blocked, and assigns an agent to p.
    local.with(|s| {
        let rid = s.items["r"].task_id.clone().unwrap();
        s.tickets.get_mut(&rid).unwrap().status = "quality_review".into();
        let bid = s.items["b"].task_id.clone().unwrap();
        s.tickets.get_mut(&bid).unwrap().status = "blocked".into();
        let pid = s.items["p"].task_id.clone().unwrap();
        s.tickets.get_mut(&pid).unwrap().assignees = vec!["hermes".into()];
    });
    // ClickUp moves them on.
    {
        let mut st = remote.0.lock().unwrap();
        st.tasks = vec![
            task("r", "y", 5_000_000),
            task("b", "y", 5_000_000),
            task("p", "z", 5_000_000),
        ];
    }
    drive(&local, &remote, &input("manual")).await;
    assert_eq!(local.ticket_of("r").unwrap().status, "quality_review");
    assert_eq!(local.ticket_of("b").unwrap().status, "blocked");
    assert_eq!(local.ticket_of("p").unwrap().status, "doing");
    for ext in ["r", "b", "p"] {
        let item = local.item_of(ext).unwrap();
        assert!(
            item.problems.iter().any(|p| p.kind == "status-held"),
            "{ext} says why"
        );
        assert_eq!(
            item.applied.status.as_deref(),
            Some("x"),
            "{ext}: held, so asked again next time"
        );
    }
    assert_eq!(
        local.ticket_of("p").unwrap().assignees,
        vec!["hermes".to_string()],
        "agents untouched"
    );
}

#[tokio::test]
async fn remap_moves_only_the_tickets_still_where_the_old_mapping_put_them() {
    let local = FakeLocal::new(&[]);
    local.with(|s| {
        let o = &mut s.sync.as_mut().unwrap().options;
        o.comments = false;
        o.attachments = false;
    });
    let remote = FakeRemote::new(vec![task("a", "qa", 10), task("b", "qa", 10)]);
    drive(&local, &remote, &input("manual")).await;
    assert_eq!(local.ticket_of("a").unwrap().status, "inbox");
    // A person moves b by hand; then the owner maps qa → doing.
    local.with(|s| {
        let bid = s.items["b"].task_id.clone().unwrap();
        s.tickets.get_mut(&bid).unwrap().status = "quality_review".into();
        let sync = s.sync.as_mut().unwrap();
        sync.status_map.insert("qa".into(), "doing".into());
        sync.map_version = 2;
        sync.remap_pending = true;
    });
    let calls = remote.calls();
    drive(&local, &remote, &input("remap")).await;
    assert_eq!(remote.calls(), calls, "a remap makes no ClickUp calls");
    assert_eq!(local.ticket_of("a").unwrap().status, "doing");
    assert_eq!(
        local.ticket_of("b").unwrap().status,
        "quality_review",
        "the person's move stands"
    );
    assert!(
        !local
            .item_of("a")
            .unwrap()
            .problems
            .iter()
            .any(|p| p.kind == "unmapped-status")
    );
    assert!(
        !local.with(|s| s.sync.as_ref().unwrap().remap_pending),
        "the version it applied clears"
    );
    assert_eq!(
        local.with(|s| s.finishes.last().unwrap().watermark_ms),
        None,
        "a remap moves no watermark"
    );
}

#[tokio::test]
async fn a_ticket_deleted_in_talaria_is_never_made_again() {
    let local = FakeLocal::new(&[("x", "doing")]);
    let remote = FakeRemote::new(vec![task("a", "x", 10)]);
    drive(&local, &remote, &input("manual")).await;
    local.with(|s| {
        let id = s.items["a"].task_id.clone().unwrap();
        s.tickets.remove(&id);
        s.items.get_mut("a").unwrap().task_id = None; // on delete set null
    });
    remote.0.lock().unwrap().tasks = vec![task("a", "x", 5_000_000)];
    drive(&local, &remote, &input("manual")).await;
    assert_eq!(local.with(|s| s.creates), 1);
}

#[tokio::test]
async fn the_sweep_marks_missing_tasks_and_the_guard_holds_a_mass_disappearance() {
    let local = FakeLocal::new(&[("x", "doing")]);
    local.with(|s| {
        let o = &mut s.sync.as_mut().unwrap().options;
        o.comments = false;
        o.attachments = false;
    });
    let all: Vec<Task> = (0..20)
        .map(|i| task(&format!("t{i:02}"), "x", 10))
        .collect();
    let remote = FakeRemote::new(all.clone());
    drive(&local, &remote, &input("manual")).await; // first sync: no sweep
    // One task deleted in ClickUp: a manual sync sweeps and marks it.
    remote.0.lock().unwrap().tasks = all[1..].to_vec();
    drive(&local, &remote, &input("manual")).await;
    assert_eq!(local.item_of("t00").unwrap().remote_state, "deleted");
    assert!(local.ticket_of("t00").is_some(), "marked, never deleted");
    assert!(local.with(|s| s.finishes.last().unwrap().swept));
    // Fifteen vanish at once: the guard marks nothing and says so.
    remote.0.lock().unwrap().tasks = all[16..].to_vec();
    drive(&local, &remote, &input("manual")).await;
    assert_eq!(local.item_of("t05").unwrap().remote_state, "active");
    assert!(
        local
            .with(|s| s.finishes.last().unwrap().problems.clone())
            .iter()
            .any(|p| p.kind == "sweep-held")
    );
}

#[tokio::test]
async fn the_sweep_picks_up_a_task_the_incremental_walk_slipped_past() {
    let local = FakeLocal::new(&[("x", "doing")]);
    local.with(|s| {
        let o = &mut s.sync.as_mut().unwrap().options;
        o.comments = false;
        o.attachments = false;
    });
    let remote = FakeRemote::new(vec![task("a", "x", 10)]);
    drive(&local, &remote, &input("manual")).await;
    // A task older than the watermark that no ticket has (an offset page
    // shifted under the first walk): only the sweep can see it.
    remote.0.lock().unwrap().tasks.push(task("b", "x", 5));
    drive(&local, &remote, &input("manual")).await;
    assert!(local.ticket_of("b").is_some(), "the sweep reconciles");
}

#[tokio::test]
async fn an_edited_comment_is_mirrored_and_a_reply_is_threaded() {
    let local = FakeLocal::new(&[("x", "doing")]);
    local.with(|s| s.sync.as_mut().unwrap().options.attachments = false);
    let remote = FakeRemote::new(vec![task("a", "x", 10)]);
    remote
        .0
        .lock()
        .unwrap()
        .comments
        .insert("a".into(), vec![comment("c1", "first", 10, 0)]);
    drive(&local, &remote, &input("manual")).await;
    {
        let mut st = remote.0.lock().unwrap();
        st.tasks = vec![task("a", "x", 5_000_000)];
        st.comments
            .insert("a".into(), vec![comment("c1", "first, edited", 10, 1)]);
        st.replies
            .insert("c1".into(), vec![comment("r1", "a reply", 15, 0)]);
    }
    drive(&local, &remote, &input("manual")).await;
    let comments = local.with(|s| s.comments.clone());
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].3, "first, edited");
    assert_eq!(local.with(|s| s.edits), 1);
    assert_eq!(
        comments[1].5.as_deref(),
        Some("m0"),
        "the reply threads under its comment"
    );
}

#[tokio::test]
async fn a_scheduled_run_on_a_paused_sync_does_nothing() {
    let local = FakeLocal::new(&[]);
    local.with(|s| s.sync.as_mut().unwrap().state = "paused".into());
    let remote = FakeRemote::new(vec![task("a", "x", 10)]);
    let r = step(&local, &remote, &input("scheduled"), &Value::Null).await;
    assert!(matches!(r, StepResult::Done { .. }));
    assert_eq!(remote.calls(), 0);
}

#[test]
fn the_overdue_flag_reads_a_fixed_clock() {
    let mut s = sync_row(&[]);
    s.next_sync_ms = Some(1_000);
    assert!(crate::status::overdue(&s, false, 1_000 + 6 * 60_000));
    assert!(
        !crate::status::overdue(&s, true, 1_000 + 6 * 60_000),
        "a live run is not overdue"
    );
    assert!(!crate::status::overdue(&s, false, 1_000 + 60_000));
    s.interval_minutes = None;
    assert!(
        !crate::status::overdue(&s, false, 1_000 + 6 * 60_000),
        "manual is never overdue"
    );
}

#[test]
fn the_fresh_checkpoint_sweeps_only_when_it_should() {
    use crate::run::{Input, Mode, fresh};
    let mut s = sync_row(&[]);
    let i = |m| Input {
        sync_id: "s".into(),
        mode: m,
        external_id: None,
    };
    assert!(
        !fresh(&i(Mode::Manual), &s, 0).sweep,
        "the first sync's walk is already full"
    );
    s.watermark_ms = Some(1);
    assert!(
        fresh(&i(Mode::Manual), &s, 0).sweep,
        "Sync now always sweeps"
    );
    assert!(
        fresh(&i(Mode::Scheduled), &s, 0).sweep,
        "never swept: sweep"
    );
    s.last_sweep_ms = Some(0);
    assert!(!fresh(&i(Mode::Scheduled), &s, 1_000).sweep);
    assert!(fresh(&i(Mode::Scheduled), &s, crate::run::SWEEP_EVERY_MS + 1).sweep);
    assert_eq!(fresh(&i(Mode::Remap), &s, 0).pass, Pass::Remap);
}

#[test]
fn the_keys_are_stable() {
    assert_eq!(crate::external_key("86a1"), "clickup:86a1");
    assert_eq!(crate::comment_key("9012"), "clickup:comment:9012");
    let _: StatusMap = StatusMap::new();
}

#[tokio::test]
async fn files_come_from_the_detail_read_never_from_a_list_page() {
    let local = FakeLocal::new(&[("x", "doing")]);
    local.with(|s| s.sync.as_mut().unwrap().options.comments = false);
    // The list page carries an `attachments` field with no URL; GET /task
    // carries the file properly.
    let listed = Task::parse(&json!({
        "id": "a", "name": "A", "status": {"status": "x"}, "date_updated": "10",
        "attachments": [{"id": "f1", "title": "notes.txt", "size": 5}]
    }))
    .unwrap();
    let detailed = Task::parse(&json!({
        "id": "a", "name": "A", "status": {"status": "x"}, "date_updated": "10",
        "attachments": [{"id": "f1", "title": "notes.txt", "size": 5, "mimetype": "text/plain",
                         "url": "https://files.example/notes.txt"}]
    }))
    .unwrap();
    let remote = FakeRemote::new(vec![listed]);
    remote
        .0
        .lock()
        .unwrap()
        .details
        .insert("a".into(), detailed);
    drive(&local, &remote, &input("manual")).await;
    let item = local.item_of("a").unwrap();
    assert_eq!(
        item.attachments["f1"].state, "done",
        "{:?}",
        item.attachments
    );
    assert_eq!(
        remote.0.lock().unwrap().downloads,
        vec!["https://files.example/notes.txt".to_string()]
    );
    assert_eq!(
        local
            .ticket_of("a")
            .unwrap()
            .attachments
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

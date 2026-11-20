// board_sync_live (see main.rs): a board syncs from a ClickUp list, end to end
// through the real router, the real runs engine and the real task engine,
// against a loopback ClickUp stand-in (TALARIA_CLICKUP_API_BASE plus
// TALARIA_FETCH_ALLOW_HOSTS). The stand-in answers only the paths the client
// dials and every assertion is on those paths — another process on this host
// may poll a loopback port's `GET /`.
//
// House rule: #[ignore]d, never CI's default job.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… TALARIA_SECRET_KEY=… \
//     cargo test -p talaria-api --test it board_sync_live:: -- --ignored

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "board-sync-live";
const BIG: i64 = 30 * 1024 * 1024;

/// The stand-in's world. Tasks are served oldest first (the client asks
/// `reverse=true`), all on page 0. `tasks` are list L1's, `tasks2` list L2's.
#[derive(Default)]
struct World {
    tasks: Vec<Value>,
    tasks2: Vec<Value>,
    archived: Vec<Value>,
    comments: HashMap<String, Vec<Value>>,
    replies: HashMap<String, Vec<Value>>,
    token_dead: bool,
    /// L1 is gone (or out of the token's sight): its reads answer 404.
    list_gone: bool,
    hits: Vec<String>,
}

struct StandIn {
    base: String,
    world: Arc<Mutex<World>>,
}

impl StandIn {
    fn hits(&self, path: &str) -> usize {
        self.world
            .lock()
            .unwrap()
            .hits
            .iter()
            .filter(|h| h.as_str() == path)
            .count()
    }
}

fn answer(world: &Mutex<World>, base: &str, path: &str, query: &str) -> (u16, Value) {
    let q: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut w = world.lock().unwrap();
    w.hits.push(path.to_string());
    if w.token_dead && path.starts_with("/api/v2/") {
        return (401, json!({"err": "Token invalid", "ECODE": "OAUTH_025"}));
    }
    if w.list_gone && path.starts_with("/api/v2/list/L1") {
        return (404, json!({"err": "List not found", "ECODE": "ITEM_013"}));
    }
    let seg: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match seg.as_slice() {
        ["api", "v2", "user"] => (
            200,
            json!({"user": {"id": 1, "username": "Sync Bot", "email": "bot@clickup.invalid"}}),
        ),
        ["api", "v2", "team"] => (200, json!({"teams": [{"id": "9001", "name": "Workspace"}]})),
        ["api", "v2", "list", lid] if *lid == "L1" || *lid == "L2" => (
            200,
            json!({
                "id": lid, "name": if *lid == "L1" { "Sprint" } else { "Backlog" },
                "task_count": if *lid == "L1" { w.tasks.len() } else { w.tasks2.len() },
                "space": {"id": "S1", "name": "Engineering"},
                "folder": {"id": "F0", "name": "hidden", "hidden": true},
                "statuses": [
                    {"status": "to do", "type": "open", "orderindex": 0, "color": "#ddd"},
                    {"status": "in progress", "type": "custom", "orderindex": 1, "color": "#49f"},
                    {"status": "qa", "type": "custom", "orderindex": 2, "color": "#fd0"},
                    {"status": "complete", "type": "closed", "orderindex": 3, "color": "#6c5"}
                ]
            }),
        ),
        ["api", "v2", "list", lid, "task"] if *lid == "L1" || *lid == "L2" => {
            let page: u32 = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(0);
            let none = Vec::new();
            let src = if *lid == "L2" {
                if q.get("archived").map(String::as_str) == Some("true") {
                    &none
                } else {
                    &w.tasks2
                }
            } else if q.get("archived").map(String::as_str) == Some("true") {
                &w.archived
            } else {
                &w.tasks
            };
            let after: i64 = q
                .get("date_updated_gt")
                .and_then(|d| d.parse().ok())
                .unwrap_or(0);
            let tasks: Vec<Value> = if page == 0 {
                src.iter()
                    .filter(|t| {
                        t["date_updated"]
                            .as_str()
                            .and_then(|d| d.parse::<i64>().ok())
                            .unwrap_or(0)
                            > after
                    })
                    .cloned()
                    .collect()
            } else {
                vec![]
            };
            (200, json!({"tasks": tasks, "last_page": true}))
        }
        ["api", "v2", "task", id] => {
            let found = w
                .tasks
                .iter()
                .chain(w.archived.iter())
                .chain(w.tasks2.iter())
                .find(|t| t["id"] == *id)
                .cloned();
            match found {
                Some(mut t) => {
                    if t.get("attachments").is_none() {
                        t["attachments"] = json!([]);
                    }
                    for a in t["attachments"].as_array_mut().unwrap() {
                        let file = a["file"].as_str().unwrap_or("").to_string();
                        a["url"] = json!(format!("{base}/files/{file}"));
                    }
                    (200, t)
                }
                None => (404, json!({"err": "Task not found", "ECODE": "ITEM_013"})),
            }
        }
        ["api", "v2", "task", id, "comment"] => {
            if q.contains_key("start_id") {
                return (200, json!({"comments": []}));
            }
            (
                200,
                json!({"comments": w.comments.get(*id).cloned().unwrap_or_default()}),
            )
        }
        ["api", "v2", "comment", id, "reply"] => (
            200,
            json!({"comments": w.replies.get(*id).cloned().unwrap_or_default()}),
        ),
        _ => (404, json!({"err": "no such path"})),
    }
}

/// HTTP/1.1, one request per connection. `/files/<name>` serves bytes.
async fn stand_in() -> StandIn {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let world: Arc<Mutex<World>> = Arc::default();
    let w = world.clone();
    let b = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let w = w.clone();
            let b = b.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|x| x == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let target = head
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_string();
                let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
                let (status, ctype, body): (u16, &str, Vec<u8>) =
                    if let Some(name) = path.strip_prefix("/files/") {
                        w.lock().unwrap().hits.push(path.to_string());
                        (
                            200,
                            "text/plain",
                            format!("contents of {name}").into_bytes(),
                        )
                    } else {
                        let (s, v) = answer(&w, &b, path, query);
                        (s, "application/json", v.to_string().into_bytes())
                    };
                let reset = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 60;
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\
                     x-ratelimit-limit: 100\r\nx-ratelimit-remaining: 99\r\nx-ratelimit-reset: {reset}\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    StandIn { base, world }
}

fn cu_task(id: &str, status: &str, closed: bool, updated: i64, extra: Value) -> Value {
    let mut t = json!({
        "id": id,
        "name": format!("ClickUp task {id}"),
        "markdown_description": format!("Description of {id}"),
        "status": {"status": status, "type": if closed { "closed" } else { "custom" }},
        "date_created": "1700000000000",
        "date_updated": updated.to_string(),
        "date_closed": if closed { json!("1700000000000") } else { Value::Null },
        "archived": false,
        "assignees": [],
        "tags": [],
        "priority": null,
        "list": {"id": "L1"},
        "url": format!("https://app.clickup.com/t/{id}"),
        "creator": {"id": 1, "username": "Dana Reyes"},
    });
    if let (Some(o), Some(e)) = (t.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    t
}

async fn wait_run(pg: &PgPool, run_id: &str) -> (String, Option<String>) {
    let end = Instant::now() + Duration::from_secs(90);
    loop {
        let (state, error): (String, Option<String>) =
            sqlx::query_as("select state, error from runs where id = $1::uuid")
                .bind(run_id)
                .fetch_one(pg)
                .await
                .unwrap();
        if matches!(state.as_str(), "done" | "error" | "cancelled") || Instant::now() > end {
            return (state, error);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn ticket(pg: &PgPool, board: &str, ext: &str) -> Value {
    let row: Option<(Value,)> = sqlx::query_as(
        "select to_jsonb(t) from tasks t where t.board_id = $1::uuid and t.external_key = $2",
    )
    .bind(board)
    .bind(format!("clickup:{ext}"))
    .fetch_optional(pg)
    .await
    .unwrap();
    row.map(|(v,)| v).unwrap_or(Value::Null)
}

async fn imported(pg: &PgPool, board: &str) -> Vec<(String, String, String, Option<String>, i64)> {
    sqlx::query_as(
        "select m.author, m.content, m.import_key, m.thread_root_id::text, \
           (trunc(extract(epoch from m.created_at) * 1000))::bigint \
         from channel_messages m join channels c on c.id = m.channel_id \
         join tasks t on t.id = c.task_id \
         where t.board_id = $1::uuid and m.author_type = 'imported' order by m.seq",
    )
    .bind(board)
    .fetch_all(pg)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore]
async fn a_board_syncs_from_a_clickup_list_end_to_end() {
    crate::support::wire_boot_seams();
    let cu = stand_in().await;
    // SAFETY: set once, before the first ClickUp call, to the values every
    // reader in this binary may see.
    unsafe {
        std::env::set_var("TALARIA_FETCH_ALLOW_HOSTS", "127.0.0.1");
        std::env::set_var(talaria_clickup::BASE_ENV, format!("{}/api/v2", cu.base));
    }
    let state = live_app_state().await;
    let pg = state.pg.clone();
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let owner_email = format!("{PREFIX}-owner-{tag}@example.invalid");
    let (owner, cookie) = minted_member_with_email(&state, PREFIX, &owner_email).await;
    let dana_email = format!("{PREFIX}-dana-{tag}@example.invalid");
    let dana =
        crate::support::person(&pg, &format!("{PREFIX}:{tag}:dana"), &dana_email, "Dana").await;
    let sam_email = format!("{PREFIX}-sam-{tag}@example.invalid");
    let _sam = crate::support::person(&pg, &format!("{PREFIX}:{tag}:sam"), &sam_email, "Sam").await;
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('Sync board', $1::uuid) returning id::text",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    for (uid, role) in [(&owner, "owner"), (&dana, "editor")] {
        sqlx::query(
            "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, $3)",
        )
        .bind(&board)
        .bind(uid)
        .bind(role)
        .execute(&pg)
        .await
        .unwrap();
    }

    {
        let mut w = cu.world.lock().unwrap();
        w.tasks = vec![
            cu_task(
                &format!("A{tag}"),
                "in progress",
                false,
                1_700_000_001_000,
                json!({
                    "priority": {"id": "2", "priority": "high"},
                    "due_date": "1767225600000",
                    "tags": [{"name": "backend"}, {"name": "x".repeat(45)}],
                    "assignees": [
                        {"id": 7, "username": "Dana", "email": dana_email},
                        {"id": 8, "username": "Sam", "email": sam_email},
                        {"id": 9, "username": "Zed", "email": format!("zed-{tag}@nowhere.invalid")}
                    ],
                    "attachments": [
                        {"id": "att-small", "title": "notes.txt", "mimetype": "text/plain", "size": 20, "file": "notes.txt"},
                        {"id": "att-big", "title": "design.mov", "mimetype": "video/quicktime", "size": BIG, "file": "design.mov"}
                    ]
                }),
            ),
            cu_task(
                &format!("B{tag}"),
                "complete",
                true,
                1_700_000_002_000,
                json!({}),
            ),
            cu_task(
                &format!("C{tag}"),
                "qa",
                false,
                1_700_000_003_000,
                json!({}),
            ),
            cu_task(
                &format!("D{tag}"),
                "to do",
                false,
                1_700_000_004_000,
                json!({"parent": format!("A{tag}"), "top_level_parent": format!("A{tag}")}),
            ),
        ];
        w.archived = vec![cu_task(
            &format!("E{tag}"),
            "to do",
            false,
            1_700_000_005_000,
            json!({"archived": true}),
        )];
        w.comments.insert(
            format!("A{tag}"),
            vec![
                json!({"id": format!("c2{tag}"), "comment_text": "second", "date": "1700000200000", "reply_count": 0,
                       "user": {"id": 7, "username": "Dana Reyes", "email": dana_email}}),
                json!({"id": format!("c1{tag}"), "comment_text": "first", "date": "1700000100000", "reply_count": "1",
                       "user": {"id": 7, "username": "Dana Reyes", "email": dana_email}}),
            ],
        );
        w.replies.insert(
            format!("c1{tag}"),
            vec![json!({"id": format!("r1{tag}"), "comment_text": "a reply", "date": "1700000150000", "reply_count": 0,
                        "user": {"id": 8, "username": "Sam"}})],
        );
    }

    // Both people hear this WHOLE board (talaria_notify::boards): had a sync
    // write raised a person's news, it would land here as board activity —
    // the "New ticket" row of every create, the move row of the remap below.
    let (status, body) = call_json(
        &state,
        "PUT",
        &format!("/api/boards/{board}/notify"),
        Some(&cookie),
        Some(json!({"preset": "all"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query(
        "insert into board_notify_prefs (user_id, board_id, preset) \
         values ($1::uuid, $2::uuid, 'all')",
    )
    .bind(&dana)
    .bind(&board)
    .execute(&pg)
    .await
    .unwrap();

    // ── connect: the token checked, the mapping validated, the first sync ──
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        Some(json!({
            "token": "pk_live_test_token_1234",
            "listId": "L1",
            "statusMap": {"In Progress": "in_progress", "to do": "inbox", "complete": "done"},
            "options": {"includeClosed": true},
            "intervalMinutes": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sync"]["credentialHint"], "1234");
    assert!(
        body.to_string().find("pk_live_test_token").is_none(),
        "the token is never answered"
    );
    let run_id = body["runId"].as_str().unwrap().to_string();
    let (state_s, err) = wait_run(&pg, &run_id).await;
    assert_eq!(state_s, "done", "{err:?}");

    // Tickets, keyed by external_key; the archived task was not imported.
    let a = ticket(&pg, &board, &format!("A{tag}")).await;
    let b = ticket(&pg, &board, &format!("B{tag}")).await;
    let c = ticket(&pg, &board, &format!("C{tag}")).await;
    let d = ticket(&pg, &board, &format!("D{tag}")).await;
    assert!(ticket(&pg, &board, &format!("E{tag}")).await.is_null());
    assert_eq!(a["status"], "in_progress");
    assert_eq!(a["priority"], "high");
    assert_eq!(a["created_by"], "clickup-sync");
    assert_eq!(a["assignees"], json!([format!("user:{dana}")]));
    assert_eq!(a["tags"], json!(["backend", "x".repeat(40)]));
    assert_eq!(
        b["status"], "done",
        "a closed task lands in the mapped done column"
    );
    assert!(
        b["approved_at"].is_null(),
        "the sync never signs anything off"
    );
    assert_eq!(c["status"], "inbox", "unmapped: the intake");
    assert_eq!(d["parent_id"], a["id"]);
    // Problems, per item.
    let problems: Vec<(Value,)> = sqlx::query_as(
        "select problems from board_sync_items where board_id = $1::uuid order by external_id",
    )
    .bind(&board)
    .fetch_all(&pg)
    .await
    .unwrap();
    let all = problems
        .iter()
        .map(|(p,)| p.to_string())
        .collect::<String>();
    for kind in [
        "unmapped-status",
        "assignee-not-member",
        "assignee-no-account",
        "attachment-too-large",
        "tags-truncated",
    ] {
        assert!(all.contains(kind), "{kind} missing from {all}");
    }
    // Comments: imported, attributed by label, original times, oldest first,
    // the reply threaded.
    let msgs = imported(&pg, &board).await;
    assert_eq!(msgs.len(), 3, "{msgs:?}");
    assert_eq!(msgs[0].1, "first");
    assert_eq!(msgs[0].0, "Dana Reyes (ClickUp)");
    assert_eq!(msgs[0].4, 1_700_000_100_000);
    assert_eq!(msgs[1].1, "a reply");
    assert!(msgs[1].3.is_some(), "the reply threads under its comment");
    assert_eq!(msgs[2].1, "second");
    // Attachments: the small one copied, the big one never fetched.
    assert_eq!(cu.hits("/files/notes.txt"), 1);
    assert_eq!(
        cu.hits("/files/design.mov"),
        0,
        "an oversized file is never downloaded"
    );
    let atts = ticket(&pg, &board, &format!("A{tag}")).await["attachments"].clone();
    assert_eq!(atts.as_array().map(|a| a.len()), Some(1), "{atts}");
    // Imported comments light no unread badge: they live only in ticket
    // rooms, which every unread surface leaves out.
    assert_eq!(
        talaria_channels::channel_unread_total(&pg, &dana)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        talaria_channels::channel_unread_total(&pg, &owner)
            .await
            .unwrap(),
        0
    );
    // Quiet: nobody was notified about sync-made or sync-assigned tickets.
    let (notes,): (i64,) = sqlx::query_as(
        "select count(*)::bigint from notifications where user_id = $1::uuid and kind in ('task-assigned', 'task-status')",
    )
    .bind(&dana)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(notes, 0);

    // ── the status: last sync, problems, no token ──────────────────────────
    let (status, st) = call_json(
        &state,
        "GET",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(st["sync"]["lastSyncedAt"].is_string());
    assert_eq!(st["sync"]["listPath"], "Engineering / Sprint");
    assert!(st["sync"]["problemCount"].as_i64().unwrap() >= 5);
    assert!(st["sync"]["liveRun"].is_null());

    // ── Sync now again: an update, never a duplicate ───────────────────────
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync/runs"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (state_s, err) = wait_run(&pg, body["runId"].as_str().unwrap()).await;
    assert_eq!(state_s, "done", "{err:?}");
    let (n,): (i64,) = sqlx::query_as(
        "select count(*)::bigint from tasks where board_id = $1::uuid and external_key is not null",
    )
    .bind(&board)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(n, 4);
    assert_eq!(imported(&pg, &board).await.len(), 3);

    // ── a ClickUp change and a Talaria edit on other fields both survive ───
    sqlx::query("update tasks set title = 'Renamed in Talaria' where board_id = $1::uuid and external_key = $2")
        .bind(&board)
        .bind(format!("clickup:A{tag}"))
        .execute(&pg)
        .await
        .unwrap();
    {
        let mut w = cu.world.lock().unwrap();
        let t = &mut w.tasks[0];
        t["priority"] = json!({"id": "1", "priority": "urgent"});
        t["date_updated"] = json!("1800000000000");
    }
    let (_, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync/runs"),
        Some(&cookie),
        None,
    )
    .await;
    wait_run(&pg, body["runId"].as_str().unwrap()).await;
    let a = ticket(&pg, &board, &format!("A{tag}")).await;
    assert_eq!(
        a["title"], "Renamed in Talaria",
        "ClickUp did not change the title"
    );
    assert_eq!(a["priority"], "urgent", "ClickUp changed the priority");

    // ── map QA: the remap moves the ticket the old mapping placed ─────────
    let (status, body) = call_json(
        &state,
        "PUT",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        Some(json!({"statusMap": {"in progress": "in_progress", "to do": "inbox", "complete": "done", "qa": "quality_review"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let end = Instant::now() + Duration::from_secs(60);
    while ticket(&pg, &board, &format!("C{tag}")).await["status"] != "quality_review"
        && Instant::now() < end
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        ticket(&pg, &board, &format!("C{tag}")).await["status"],
        "quality_review"
    );
    // A SYNC WRITE IS A SYSTEM ACTOR'S MIRROR: four creates, an assignment,
    // updates and a move, and neither whole-board listener holds a row about
    // any of it — no board activity, no per-ticket row. (The fan-outs are
    // detached: give a leaked one time to land before saying it did not.)
    tokio::time::sleep(Duration::from_millis(1500)).await;
    for uid in [&owner, &dana] {
        let (n,): (i64,) = sqlx::query_as(
            "select count(*)::bigint from notifications where user_id = $1::uuid \
             and kind in ('board-activity', 'task-status', 'task-assigned') \
             and (href like $2 or href = $3)",
        )
        .bind(uid)
        .bind(format!("/boards/{board}/%"))
        .bind(format!("/boards/{board}"))
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(n, 0, "a sync write raised a row for {uid}");
    }

    // ── a ticket deleted in Talaria is never made again ────────────────────
    sqlx::query("delete from tasks where board_id = $1::uuid and external_key = $2")
        .bind(&board)
        .bind(format!("clickup:C{tag}"))
        .execute(&pg)
        .await
        .unwrap();
    let (_, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync/runs"),
        Some(&cookie),
        None,
    )
    .await;
    wait_run(&pg, body["runId"].as_str().unwrap()).await;
    assert!(ticket(&pg, &board, &format!("C{tag}")).await.is_null());

    // ── ClickUp deletes a task: marked, never deleted ──────────────────────
    cu.world
        .lock()
        .unwrap()
        .tasks
        .retain(|t| t["id"] != format!("B{tag}"));
    let (_, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync/runs"),
        Some(&cookie),
        None,
    )
    .await;
    wait_run(&pg, body["runId"].as_str().unwrap()).await;
    let (remote,): (String,) =
        sqlx::query_as("select remote_state from board_sync_items where external_id = $1")
            .bind(format!("B{tag}"))
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(remote, "deleted");
    assert!(!ticket(&pg, &board, &format!("B{tag}")).await.is_null());

    // ── the token dies: needs_credential, ONE grouped notice ───────────────
    cu.world.lock().unwrap().token_dead = true;
    for _ in 0..2 {
        let (_, body) = call_json(
            &state,
            "POST",
            &format!("/api/boards/{board}/sync/runs"),
            Some(&cookie),
            None,
        )
        .await;
        if let Some(id) = body["runId"].as_str() {
            let (s, _) = wait_run(&pg, id).await;
            assert_eq!(s, "error");
        }
    }
    let (sync_state,): (String,) = sqlx::query_as(
        "select state from board_syncs where board_id = $1::uuid and state <> 'disconnected'",
    )
    .bind(&board)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(sync_state, "needs_credential");
    let (notices,): (i64,) = sqlx::query_as(
        "select count(*)::bigint from notifications where user_id = $1::uuid and kind = 'board-sync'",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        notices, 1,
        "one notice for the owner, not one per failed run"
    );
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync/runs"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "Sync now says a credential is needed"
    );

    // ── replace the credential: active again, the mapping kept ─────────────
    cu.world.lock().unwrap().token_dead = false;
    let (status, body) = call_json(
        &state,
        "PUT",
        &format!("/api/boards/{board}/sync/credential"),
        Some(&cookie),
        Some(json!({"token": "pk_replacement_token_5678"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sync"]["state"], "active");
    assert_eq!(body["sync"]["statusMap"]["qa"], "quality_review");
    if let Some((id, _)) = talaria_board_sync::live_run(&pg, &board).await.unwrap() {
        wait_run(&pg, &id).await;
    }

    // ── an editor may Sync now but not edit; a stranger sees nothing ───────
    let (dana_id, dana_cookie) = minted_member_with_email(
        &state,
        PREFIX,
        &format!("{PREFIX}-ed-{tag}@example.invalid"),
    )
    .await;
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'editor')",
    )
    .bind(&board)
    .bind(&dana_id)
    .execute(&pg)
    .await
    .unwrap();
    let (status, _) = call_json(
        &state,
        "PUT",
        &format!("/api/boards/{board}/sync"),
        Some(&dana_cookie),
        Some(json!({"paused": true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, stranger) =
        minted_member_with_email(&state, PREFIX, &format!("{PREFIX}-x-{tag}@example.invalid"))
            .await;
    let (status, _) = call_json(
        &state,
        "GET",
        &format!("/api/boards/{board}/sync"),
        Some(&stranger),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // ── disconnect: the tickets stay, the credential goes ──────────────────
    let (status, _) = call_json(
        &state,
        "DELETE",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (sealed,): (Option<String>,) = sqlx::query_as(
        "select credential_sealed from board_syncs where board_id = $1::uuid order by created_at desc limit 1",
    )
    .bind(&board)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(sealed.is_none());
    assert!(!ticket(&pg, &board, &format!("A{tag}")).await.is_null());
    let (_, st) = call_json(
        &state,
        "GET",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        None,
    )
    .await;
    assert!(st["sync"].is_null());
    assert_eq!(st["disconnected"]["listPath"], "Engineering / Sprint");
    let a_id = ticket(&pg, &board, &format!("A{tag}")).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/tasks/{a_id}/resync"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no live connection, no re-sync"
    );

    // ── the same board, another list: the first list's tickets stay out ────
    cu.world.lock().unwrap().tasks2 = vec![cu_task(
        &format!("F{tag}"),
        "to do",
        false,
        1_700_000_006_000,
        json!({"list": {"id": "L2"}}),
    )];
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        Some(json!({
            "token": "pk_live_test_token_1234",
            "listId": "L2",
            "statusMap": {"to do": "inbox"},
            "intervalMinutes": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (state_s, err) = wait_run(&pg, body["runId"].as_str().unwrap()).await;
    assert_eq!(state_s, "done", "{err:?}");
    // Sync now sweeps. A's ticket came from L1, so L2's sweep must not read
    // it as a task moved out of L2.
    let (_, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board}/sync/runs"),
        Some(&cookie),
        None,
    )
    .await;
    let (state_s, err) = wait_run(&pg, body["runId"].as_str().unwrap()).await;
    assert_eq!(state_s, "done", "{err:?}");
    assert!(!ticket(&pg, &board, &format!("F{tag}")).await.is_null());
    let (remote, list): (String, String) = sqlx::query_as(
        "select i.remote_state, s.list_id from board_sync_items i \
         join board_syncs s on s.id = i.sync_id where i.external_id = $1",
    )
    .bind(format!("A{tag}"))
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(remote, "active", "another list's sweep marked it");
    assert_eq!(list, "L1", "it stays with the connection that synced it");
    let (_, st) = call_json(
        &state,
        "GET",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        None,
    )
    .await;
    assert!(
        !st["sync"]["problems"].to_string().contains("design.mov"),
        "the old list's problems are not this connection's: {}",
        st["sync"]["problems"]
    );
    let (status, _) = call_json(
        &state,
        "DELETE",
        &format!("/api/boards/{board}/sync"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // ── another board takes L1 over once the first has let go ──────────────
    let (board2,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('Second sync board', $1::uuid) returning id::text",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board2)
    .bind(&owner)
    .execute(&pg)
    .await
    .unwrap();
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/boards/{board2}/sync"),
        Some(&cookie),
        Some(json!({
            "token": "pk_live_test_token_1234",
            "listId": "L1",
            "statusMap": {"in progress": "in_progress", "to do": "inbox", "complete": "done"},
            "intervalMinutes": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (state_s, err) = wait_run(&pg, body["runId"].as_str().unwrap()).await;
    assert_eq!(state_s, "done", "{err:?}");
    // C's ticket was deleted on the first board: imported here afresh.
    assert!(
        !ticket(&pg, &board2, &format!("C{tag}")).await.is_null(),
        "a task whose ticket the first board deleted imports on the second"
    );
    // A and D still have tickets on the first board: held there, and said so.
    assert!(ticket(&pg, &board2, &format!("A{tag}")).await.is_null());
    let (sync2, run_problems): (String, Value) = sqlx::query_as(
        "select id::text, run_problems from board_syncs where board_id = $1::uuid and state <> 'disconnected'",
    )
    .bind(&board2)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        run_problems.to_string().contains("other-board"),
        "{run_problems}"
    );

    // ── the list goes: one notice, however many runs stop on it ────────────
    cu.world.lock().unwrap().list_gone = true;
    for _ in 0..2 {
        let (status, body) = call_json(
            &state,
            "POST",
            &format!("/api/boards/{board2}/sync/runs"),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (s, _) = wait_run(&pg, body["runId"].as_str().unwrap()).await;
        assert_eq!(s, "error");
    }
    let (rows, count): (i64, i64) = sqlx::query_as(
        "select count(*)::bigint, coalesce(sum(count), 0)::bigint from notifications \
         where user_id = $1::uuid and kind = 'board-sync' and group_key = $2",
    )
    .bind(&owner)
    .bind(format!("board-sync:{sync2}"))
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        (rows, count),
        (1, 1),
        "one notice for a list that stays gone, not a re-alert per run"
    );
    let (sync_state, run_problems): (String, Value) =
        sqlx::query_as("select state, run_problems from board_syncs where id = $1::uuid")
            .bind(&sync2)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(
        sync_state, "active",
        "it resumes on its own once the list is back"
    );
    assert!(
        run_problems.to_string().contains("list-gone"),
        "{run_problems}"
    );
    cu.world.lock().unwrap().list_gone = false;
}

#[tokio::test]
#[ignore]
async fn an_imported_message_lands_once_per_import_key() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let owner = crate::support::person(
        &pg,
        &format!("{PREFIX}:{tag}:o"),
        &format!("{PREFIX}-o-{tag}@example.invalid"),
        "O",
    )
    .await;
    let (channel,): (String,) =
        sqlx::query_as("insert into channels (name, created_by) values ('import test', $1::uuid) returning id::text")
            .bind(&owner)
            .fetch_one(&pg)
            .await
            .unwrap();
    let notify = talaria_notify::NotifyDeps::publishing(pg.clone(), None);
    let key = format!("clickup:comment:{tag}");
    let first = talaria_channels::insert_imported_message(
        &notify,
        &channel,
        "Dana (ClickUp)",
        "hi",
        Some(1_700_000_000_000),
        None,
        &key,
    )
    .await
    .unwrap();
    let again = talaria_channels::insert_imported_message(
        &notify,
        &channel,
        "Dana (ClickUp)",
        "hi",
        Some(1_700_000_000_000),
        None,
        &key,
    )
    .await
    .unwrap();
    assert!(first.inserted);
    assert!(!again.inserted);
    assert_eq!(first.id, again.id);
    assert!(
        talaria_channels::update_imported_message(&notify, &channel, &key, "hi, edited")
            .await
            .unwrap()
    );
    assert!(
        !talaria_channels::update_imported_message(&notify, &channel, &key, "hi, edited")
            .await
            .unwrap()
    );
    let (n, author_type, at): (i64, String, i64) = sqlx::query_as(
        "select (count(*) over ())::bigint, author_type, (trunc(extract(epoch from created_at) * 1000))::bigint \
         from channel_messages where channel_id = $1::uuid",
    )
    .bind(&channel)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(author_type, "imported");
    assert_eq!(at, 1_700_000_000_000, "the original time");
}

#[tokio::test]
#[ignore]
async fn the_secret_inventory_lists_and_clears_a_sync_credential() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let owner = crate::support::person(
        &pg,
        &format!("{PREFIX}:{tag}:s"),
        &format!("{PREFIX}-s-{tag}@example.invalid"),
        "S",
    )
    .await;
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ($1, $2::uuid) returning id::text",
    )
    .bind(format!("Secret board {tag}"))
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    let sb = state.secretbox().await.unwrap();
    let sealed = sb.seal("pk_secret_inventory").unwrap();
    let (sync_id,): (String,) = sqlx::query_as(
        "insert into board_syncs (board_id, list_id, credential_sealed, credential_hint, state) \
         values ($1::uuid, 'L', $2, 'tory', 'active') returning id::text",
    )
    .bind(&board)
    .bind(&sealed)
    .fetch_one(&pg)
    .await
    .unwrap();
    let health = talaria_secret_health::secret_health(&pg, &sb, &state.cfg.secret_root).await;
    let id = format!("board-sync:{sync_id}");
    let row = health["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .cloned()
        .expect("the sync credential is listed");
    assert_eq!(row["state"], "ok");
    assert_eq!(row["group"], "integrations");
    assert!(row.to_string().find("pk_secret_inventory").is_none());
    assert!(talaria_secret_health::clear_secret(&pg, &id).await.unwrap());
    let (state_s, cipher): (String, Option<String>) =
        sqlx::query_as("select state, credential_sealed from board_syncs where id = $1::uuid")
            .bind(&sync_id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(state_s, "needs_credential");
    assert!(cipher.is_none());
}

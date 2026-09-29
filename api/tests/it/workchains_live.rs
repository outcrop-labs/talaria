// Live-DB proof of the workchain routes (cargo test -- --ignored). Every
// guarantee this file pins is one a unit test cannot vouch for, because it
// IS a Postgres rule or a router gate, not Rust: the derived
// done/head/ready/blocked states read the board's real status categories,
// the one-chain
// invariant holds under the real unique index, the task-delete cascade
// removes the step, the cross-board refusal answers 400, the reorder and
// step-delete keep the chain's order honest, and deleting a chain leaves
// its tickets standing. All of it runs through the REAL router with minted
// sessions — the stack a browser rides.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test it workchains_live:: -- --ignored

use axum::body::Body;
use axum::http::Request;
use serde_json::Value;
use sqlx::postgres::PgPool;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

use crate::support;

/// Real services, the same ones the process boots with — Redis carries the
/// minted sessions.
async fn app_state() -> AppState {
    // The engine's cross-crate edges (GET_TASK) are boot-injected in
    // production by register_all; a test binary boots none of that, so the
    // seams are set here, exactly the way the composition root sets them.
    support::wire_boot_seams();
    static TRACE: std::sync::Once = std::sync::Once::new();
    TRACE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .with_test_writer()
            .try_init();
    });
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL (source ui/.env)"),
        std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)"),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

/// Everything hangs off the users rows; deleting them cascades boards →
/// tasks → workchains → steps, the direction production deletes run. Each
/// test gets its own tag so concurrent runs never delete each other's
/// boards.
async fn reset(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where email like $1")
        .bind(format!("workchains-live-{tag}-%@link-test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

async fn user_row(pg: &PgPool, email: &str, name: &str) -> SessionUser {
    let sub = format!("workchains-live:{email}");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') \
         returning id::text",
    )
    .bind(&sub)
    .bind(email)
    .bind(name)
    .fetch_one(pg)
    .await
    .unwrap();
    SessionUser {
        id,
        sub,
        email: Some(email.to_string()),
        name: Some(name.to_string()),
        picture: None,
        role: "member".into(),
        provider: "google".into(),
    }
}

struct Fixture {
    board_id: String,
    other_board_id: String,
    owner: SessionUser,
    viewer: SessionUser,
    stranger: SessionUser,
}

async fn fixture(state: &AppState, tag: &str) -> Fixture {
    let pg = &state.pg;
    reset(pg, tag).await;
    let owner = user_row(
        pg,
        &format!("workchains-live-{tag}-owner@link-test.invalid"),
        "Workchains Owner",
    )
    .await;
    let viewer = user_row(
        pg,
        &format!("workchains-live-{tag}-viewer@link-test.invalid"),
        "Workchains Viewer",
    )
    .await;
    let stranger = user_row(
        pg,
        &format!("workchains-live-{tag}-stranger@link-test.invalid"),
        "Stranger",
    )
    .await;
    let (board_id,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('workchains live', $1::uuid) \
         returning id::text",
    )
    .bind(&owner.id)
    .fetch_one(pg)
    .await
    .unwrap();
    // The owner's board_members row is the leg access reads — boards.owner_id
    // alone is display truth (the ticket-rooms suite's own comment).
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board_id)
    .bind(&owner.id)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'viewer')",
    )
    .bind(&board_id)
    .bind(&viewer.id)
    .execute(pg)
    .await
    .unwrap();
    let (other_board_id,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('workchains other', $1::uuid) \
         returning id::text",
    )
    .bind(&owner.id)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&other_board_id)
    .bind(&owner.id)
    .execute(pg)
    .await
    .unwrap();
    Fixture {
        board_id,
        other_board_id,
        owner,
        viewer,
        stranger,
    }
}

/// A ticket on the board, in the named status.
async fn ticket(pg: &PgPool, board_id: &str, title: &str, status: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, title, status, priority, created_by) \
         values ($1::uuid, $2, $3, 'medium', 'user') returning id::text",
    )
    .bind(board_id)
    .bind(title)
    .bind(status)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// One request through the REAL router with a minted session cookie.
async fn call(
    state: &AppState,
    sid: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", format!("talaria_session={sid}"));
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = routes::router(state.clone())
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn sid(state: &AppState, user: &SessionUser) -> String {
    create_session(state, user).await.unwrap()
}

async fn create_chain(state: &AppState, owner_sid: &str, board_id: &str, name: &str) -> String {
    let (status, body) = call(
        state,
        owner_sid,
        "POST",
        &format!("/api/boards/{board_id}/workchains"),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, 200, "chain create failed: {body}");
    body["workchain"]["id"]
        .as_str()
        .expect("create returns the id")
        .to_string()
}

/// The chain's step order, as task ids by position.
async fn chain_order(pg: &PgPool, chain_id: &str) -> Vec<String> {
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "select task_id::text, position from task_workchain_steps \
         where workchain_id = $1::uuid order by position",
    )
    .bind(chain_id)
    .fetch_all(pg)
    .await
    .unwrap();
    rows.into_iter().map(|(t, _)| t).collect()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn create_and_list_with_derived_states() {
    let state = app_state().await;
    let f = fixture(&state, "list").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    // Three tickets: one done, one live, one still queued. The board has no
    // board_statuses rows, so DEFAULTS answer — 'done' is the done column
    // and everything else is live.
    let done_task = ticket(pg, &f.board_id, "Ship it", "done").await;
    let live_task = ticket(pg, &f.board_id, "In flight", "in_progress").await;
    let queued_task = ticket(pg, &f.board_id, "Queued", "inbox").await;

    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/boards/{}/workchains", f.board_id),
        Some(serde_json::json!({ "name": "Release train" })),
    )
    .await;
    assert_eq!(status, 200, "create failed: {body}");
    let chain_id = body["workchain"]["id"]
        .as_str()
        .expect("create returns the workchain id")
        .to_string();
    assert_eq!(body["workchain"]["name"], "Release train");
    assert_eq!(body["workchain"]["position"], 0);
    assert_eq!(body["workchain"]["steps"].as_array().map(Vec::len), Some(0));

    for task_id in [&done_task, &live_task, &queued_task] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain_id}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    let (status, body) = call(
        &state,
        &owner,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let chains = body["workchains"].as_array().expect("the list answers");
    assert_eq!(chains.len(), 1);
    let steps = chains[0]["steps"].as_array().expect("steps ride along");
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0]["state"], "done");
    assert_eq!(steps[0]["taskId"], done_task);
    assert_eq!(steps[1]["state"], "head");
    assert_eq!(
        steps[2]["state"], "blocked",
        "a live predecessor blocks its successor — the AND-join waits"
    );
    // The summaries a chain card renders.
    assert_eq!(steps[1]["title"], "In flight");
    assert_eq!(steps[1]["status"], "in_progress");

    // A viewer (read-only member) may LIST; a stranger may not even see the
    // board — 403, the family's grammar.
    let viewer_sid = sid(&state, &f.viewer).await;
    let (status, _) = call(
        &state,
        &viewer_sid,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200, "a viewer member may read the chains");

    let stranger_sid = sid(&state, &f.stranger).await;
    let (status, _) = call(
        &state,
        &stranger_sid,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 403, "a stranger cannot read the chains");

    reset(pg, "list").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn one_chain_per_task_is_refused_and_enforced() {
    let state = app_state().await;
    let f = fixture(&state, "one-chain").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    let task = ticket(pg, &f.board_id, "Pinned", "inbox").await;
    let chain_a = create_chain(&state, &owner, &f.board_id, "Chain A").await;
    let chain_b = create_chain(&state, &owner, &f.board_id, "Chain B").await;

    let (status, _) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain_a}/steps"),
        Some(serde_json::json!({ "taskId": task })),
    )
    .await;
    assert_eq!(status, 200, "the first chain takes the task");

    // The friendly refusal: ANY other chain, 409.
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain_b}/steps"),
        Some(serde_json::json!({ "taskId": task })),
    )
    .await;
    assert_eq!(status, 409, "a second chain must be refused: {body}");

    // The index is the truth under a race: a direct second insert dies.
    let dup = sqlx::query(
        "insert into task_workchain_steps (workchain_id, task_id, position) \
         values ($1::uuid, $2::uuid, 0)",
    )
    .bind(&chain_b)
    .bind(&task)
    .execute(pg)
    .await;
    assert!(
        dup.is_err(),
        "the one-chain index did not refuse a duplicate"
    );

    // Removing the step frees the task for the next chain.
    let (status, _) = call(
        &state,
        &owner,
        "DELETE",
        &format!("/api/workchains/{chain_a}/steps/{task}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain_b}/steps"),
        Some(serde_json::json!({ "taskId": task })),
    )
    .await;
    assert_eq!(status, 200, "a freed task may join another chain");

    reset(pg, "one-chain").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn deleting_a_task_cascades_its_step_away() {
    let state = app_state().await;
    let f = fixture(&state, "cascade").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    let gone = ticket(pg, &f.board_id, "Doomed", "inbox").await;
    let stays = ticket(pg, &f.board_id, "Survivor", "inbox").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Cascade").await;
    for task_id in [&gone, &stays] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    sqlx::query("delete from tasks where id = $1::uuid")
        .bind(&gone)
        .execute(pg)
        .await
        .unwrap();

    let (status, body) = call(
        &state,
        &owner,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let steps = body["workchains"][0]["steps"]
        .as_array()
        .expect("steps ride along");
    assert_eq!(steps.len(), 1, "the deleted task's step must be gone");
    assert_eq!(steps[0]["taskId"], stays);

    reset(pg, "cascade").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_cross_board_task_is_refused() {
    let state = app_state().await;
    let f = fixture(&state, "cross-board").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    let foreign = ticket(pg, &f.other_board_id, "Not ours", "inbox").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Home chain").await;

    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/steps"),
        Some(serde_json::json!({ "taskId": foreign })),
    )
    .await;
    assert_eq!(status, 400, "a cross-board task must be refused: {body}");

    let steps = chain_order(pg, &chain).await;
    assert!(steps.is_empty(), "the refused step must not have landed");

    reset(pg, "cross-board").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn reorder_and_step_delete_keep_the_order_honest() {
    let state = app_state().await;
    let f = fixture(&state, "reorder").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    let a = ticket(pg, &f.board_id, "A", "inbox").await;
    let b = ticket(pg, &f.board_id, "B", "inbox").await;
    let c = ticket(pg, &f.board_id, "C", "inbox").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Reorderable").await;
    for task_id in [&a, &b, &c] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    // Reverse the chain.
    let (status, body) = call(
        &state,
        &owner,
        "PATCH",
        &format!("/api/workchains/{chain}"),
        Some(serde_json::json!({
            "positions": [
                { "taskId": c, "position": 0 },
                { "taskId": b, "position": 1 },
                { "taskId": a, "position": 2 },
            ]
        })),
    )
    .await;
    assert_eq!(status, 200, "reorder failed: {body}");
    assert_eq!(
        chain_order(pg, &chain).await,
        vec![c.clone(), b.clone(), a.clone()]
    );

    // An order naming a task that is not in the chain refuses the WHOLE
    // write — the positions already sent must not land.
    let outsider = ticket(pg, &f.board_id, "Outsider", "inbox").await;
    let (status, _) = call(
        &state,
        &owner,
        "PATCH",
        &format!("/api/workchains/{chain}"),
        Some(serde_json::json!({
            "positions": [ { "taskId": outsider, "position": 0 } ]
        })),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        chain_order(pg, &chain).await,
        vec![c.clone(), b.clone(), a.clone()],
        "the refused reorder must change nothing"
    );

    // Rename and pause ride the same PATCH.
    let (status, body) = call(
        &state,
        &owner,
        "PATCH",
        &format!("/api/workchains/{chain}"),
        Some(serde_json::json!({ "name": "Renamed", "paused": true })),
    )
    .await;
    assert_eq!(status, 200, "patch failed: {body}");
    let (name, paused): (String, bool) =
        sqlx::query_as("select name, paused from task_workchains where id = $1::uuid")
            .bind(&chain)
            .fetch_one(pg)
            .await
            .unwrap();
    assert_eq!(name, "Renamed");
    assert!(paused);

    // A viewer cannot write: 403.
    let viewer_sid = sid(&state, &f.viewer).await;
    let (status, _) = call(
        &state,
        &viewer_sid,
        "PATCH",
        &format!("/api/workchains/{chain}"),
        Some(serde_json::json!({ "name": "Nope" })),
    )
    .await;
    assert_eq!(status, 403, "a viewer cannot manage workchains");

    // Step delete: the middle ticket leaves, the task itself stands.
    let (status, _) = call(
        &state,
        &owner,
        "DELETE",
        &format!("/api/workchains/{chain}/steps/{b}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(chain_order(pg, &chain).await, vec![c.clone(), a.clone()]);
    let b_alive: Option<(String,)> = sqlx::query_as("select title from tasks where id = $1::uuid")
        .bind(&b)
        .fetch_optional(pg)
        .await
        .unwrap();
    assert_eq!(b_alive.map(|(t,)| t), Some("B".to_string()));

    // Insert-after: a new ticket wedges after the FIRST step, not the tail.
    let wedged = ticket(pg, &f.board_id, "Wedged", "inbox").await;
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/steps"),
        Some(serde_json::json!({ "taskId": wedged, "after": c })),
    )
    .await;
    assert_eq!(status, 200, "insert-after failed: {body}");
    assert_eq!(chain_order(pg, &chain).await, vec![c, wedged, a]);

    // `after` naming a task that is not in this chain refuses.
    let (status, _) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/steps"),
        Some(serde_json::json!({ "taskId": outsider, "after": b })),
    )
    .await;
    assert_eq!(status, 400, "an after-name outside the chain must refuse");

    reset(pg, "reorder").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn deleting_a_chain_leaves_its_tasks_standing() {
    let state = app_state().await;
    let f = fixture(&state, "chain-delete").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    let a = ticket(pg, &f.board_id, "Stands", "inbox").await;
    let b = ticket(pg, &f.board_id, "Also stands", "inbox").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Doomed chain").await;
    for task_id in [&a, &b] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    let (status, body) = call(
        &state,
        &owner,
        "DELETE",
        &format!("/api/workchains/{chain}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "chain delete failed: {body}");

    let chains: Vec<(String,)> =
        sqlx::query_as("select id::text from task_workchains where id = $1::uuid")
            .bind(&chain)
            .fetch_all(pg)
            .await
            .unwrap();
    assert!(chains.is_empty(), "the chain must be gone");
    for (task_id, title) in [(&a, "Stands"), (&b, "Also stands")] {
        let alive: Option<(String,)> =
            sqlx::query_as("select title from tasks where id = $1::uuid")
                .bind(task_id)
                .fetch_optional(pg)
                .await
                .unwrap();
        assert_eq!(
            alive.map(|(t,)| t),
            Some(title.to_string()),
            "deleting a chain must not touch its tickets"
        );
    }

    // And the freed tasks may join a new chain — the v1 invariant is per
    // task EXISTENCE in a chain, not a memory of the old one.
    let fresh = create_chain(&state, &owner, &f.board_id, "Fresh").await;
    let (status, _) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{fresh}/steps"),
        Some(serde_json::json!({ "taskId": a })),
    )
    .await;
    assert_eq!(status, 200, "a freed task may join a new chain");

    reset(pg, "chain-delete").await;
}

// ── The handoff engine + the workchain-aware heartbeat ──────────────────────

/// A ticket with assignees — the engine and the heartbeat both key on them.
async fn ticket_with(
    pg: &PgPool,
    board_id: &str,
    title: &str,
    status: &str,
    assignees: serde_json::Value,
) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, title, status, priority, created_by, assignees) \
         values ($1::uuid, $2, $3, 'medium', 'user', $4::jsonb) returning id::text",
    )
    .bind(board_id)
    .bind(title)
    .bind(status)
    .bind(assignees)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// The engine's notification writes are detached (they must never cost the
/// ticket write), so a test asserts on them by POLLING — bounded, with the
/// query the assertion wants to run on every attempt.
async fn await_notification(pg: &PgPool, user_id: &str, kind: &str) -> Option<(String, String)> {
    for _ in 0..50 {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "select title, href from notifications \
             where user_id = $1::uuid and kind = $2 and read_at is null",
        )
        .bind(user_id)
        .bind(kind)
        .fetch_all(pg)
        .await
        .unwrap();
        if let Some(row) = rows.first() {
            return Some(row.clone());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    None
}

/// A fleet agent: an agent_defs row (the model names no real provider, so
/// nothing dispatches it), a minted per-agent key, and a board grant — the
/// three things the heartbeat's policy gate asks for. Returns (agent id,
/// model, key).
async fn fleet_agent(state: &AppState, board_id: &str, tag: &str) -> (String, String, String) {
    let pg = &state.pg;
    // slug and model are each UNIQUE on agent_defs; both carry a uuid so a
    // rerun of the suite cannot collide with a prior run's leftovers.
    let model = format!("workchains-live-{tag}-{}", uuid::Uuid::new_v4());
    let slug = format!("workchains-live-{tag}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'personal', $2, 'Workchains Live Agent') returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .fetch_one(pg)
    .await
    .unwrap();
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(pg, &sb, &id)
        .await
        .expect("agent key mints");
    let (fleet_id,): (String,) =
        sqlx::query_as("insert into fleet_agents (name) values ($1) returning id::text")
            .bind(&model)
            .fetch_one(pg)
            .await
            .expect("fleet_agents row the insert just wrote");
    sqlx::query("insert into board_agents (board_id, agent_model) values ($1::uuid, $2)")
        .bind(board_id)
        .bind(&model)
        .execute(pg)
        .await
        .expect("board_agents row");
    let _ = id;
    // The heartbeat (and every fleet-plane route) names the agent by
    // fleet_agents.id — the registry the fleet renders from — not by
    // agent_defs.id. Heartbeating with the defs id 404s as "unknown agent".
    (fleet_id, model, key)
}

/// One heartbeat through the REAL router, with the agent's own key — the
/// exact credential a container presents.
async fn heartbeat(state: &AppState, agent_id: &str, key: &str) -> (u16, Value) {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/agents/{agent_id}/heartbeat"))
                .header("x-api-key", key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// The heartbeat's work item ids — what the ordering guarantee is asserted
/// against.
fn work_item_ids(body: &Value) -> Vec<String> {
    body["work_items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|i| i["id"].as_str().expect("feed items carry ids").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn done_hands_off_to_a_human_head_with_a_notification() {
    let state = app_state().await;
    let f = fixture(&state, "turn").await;
    let pg = &state.pg;
    let owner = sid(&state, &f.owner).await;

    // A assigned to the owner, B assigned to the viewer: when A is signed
    // off, B becomes the head and the viewer hears it is their turn.
    let a = ticket_with(
        pg,
        &f.board_id,
        "Write the brief",
        "in_progress",
        serde_json::json!([format!("user:{}", f.owner.id)]),
    )
    .await;
    let b = ticket_with(
        pg,
        &f.board_id,
        "Review the draft",
        "in_progress",
        serde_json::json!([format!("user:{}", f.viewer.id)]),
    )
    .await;
    let chain = create_chain(&state, &owner, &f.board_id, "Handoff").await;
    for task_id in [&a, &b] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    // No turn row before the sign-off — the engine fires on the write.
    let (status, body) = call(
        &state,
        &owner,
        "PUT",
        &format!("/api/tasks/{a}"),
        Some(serde_json::json!({ "status": "done" })),
    )
    .await;
    assert_eq!(status, 200, "done write failed: {body}");

    // B is the head now, and the viewer (a human assignee) is told.
    let row = await_notification(pg, &f.viewer.id, "workchain_turn").await;
    let (title, href) = row.expect("the new head's human assignee gets a turn row");
    assert!(
        title.starts_with("It's your turn: "),
        "the turn title names the ticket: {title}"
    );
    assert!(
        title.contains("Review the draft"),
        "title carries B: {title}"
    );
    assert_eq!(href, format!("/boards/{}/{}", f.board_id, b));

    // The owner, who signed off, gets no turn row — the notification is the
    // HANDOFF, not an echo of the move.
    let rows: Vec<(String,)> = sqlx::query_as(
        "select 1 from notifications where user_id = $1::uuid and kind = 'workchain_turn'",
    )
    .bind(&f.owner.id)
    .fetch_all(pg)
    .await
    .unwrap();
    assert!(rows.is_empty(), "the actor gets no turn row");

    reset(pg, "turn").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_heartbeat_hides_blocked_steps_and_serves_the_ready_head() {
    let state = app_state().await;
    let f = fixture(&state, "ordering").await;
    let owner = sid(&state, &f.owner).await;
    let pg = &state.pg;
    let (agent_id, _model, key) = fleet_agent(&state, &f.board_id, "ordering").await;

    // Both steps belong to the agent; only A is servable while it is live.
    let a = ticket_with(
        pg,
        &f.board_id,
        "First",
        "in_progress",
        serde_json::json!([_model]),
    )
    .await;
    let b = ticket_with(
        pg,
        &f.board_id,
        "Second",
        "in_progress",
        serde_json::json!([_model]),
    )
    .await;
    let chain = create_chain(&state, &owner, &f.board_id, "Ordering").await;
    for task_id in [&a, &b] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    // THE CORE ORDERING GUARANTEE: the agent is ASSIGNED to B, B's column
    // is a working column, and still B is not in the feed while A is live.
    let (status, body) = heartbeat(&state, &agent_id, &key).await;
    assert_eq!(status, 200, "heartbeat failed: {body}");
    let ids = work_item_ids(&body);
    assert_eq!(ids, vec![a.clone()], "only the head may be served");
    assert_eq!(
        body["work_items"][0]["workchainReady"],
        Value::Bool(true),
        "the ready head carries its flag"
    );

    // Sign A off; B becomes the head and enters the feed flagged.
    let (status, body) = call(
        &state,
        &owner,
        "PUT",
        &format!("/api/tasks/{a}"),
        Some(serde_json::json!({ "status": "done" })),
    )
    .await;
    assert_eq!(status, 200, "done write failed: {body}");
    let (status, body) = heartbeat(&state, &agent_id, &key).await;
    assert_eq!(status, 200, "heartbeat failed: {body}");
    let ids = work_item_ids(&body);
    assert_eq!(ids, vec![b.clone()], "the new head is served");
    assert_eq!(
        body["work_items"][0]["workchainReady"],
        Value::Bool(true),
        "the new head is flagged as chain-ready"
    );

    // Sign B off too: nothing from this chain is left to serve.
    let (status, body) = call(
        &state,
        &owner,
        "PUT",
        &format!("/api/tasks/{b}"),
        Some(serde_json::json!({ "status": "done" })),
    )
    .await;
    assert_eq!(status, 200, "done write failed: {body}");
    let (status, body) = heartbeat(&state, &agent_id, &key).await;
    assert_eq!(status, 200, "heartbeat failed: {body}");
    let ids = work_item_ids(&body);
    assert!(
        !ids.contains(&b) && !ids.contains(&a),
        "an all-done chain serves nothing: {ids:?}"
    );

    reset(pg, "ordering").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn failed_pauses_the_chain_and_tells_its_creator() {
    let state = app_state().await;
    let f = fixture(&state, "pause").await;
    let owner = sid(&state, &f.owner).await;
    let pg = &state.pg;

    let a = ticket(pg, &f.board_id, "Breaks", "in_progress").await;
    let b = ticket(pg, &f.board_id, "Waits behind", "in_progress").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Paused train").await;
    for task_id in [&a, &b] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    // 'failed' is an off-board terminal: the chain pauses, the creator hears.
    let (status, body) = call(
        &state,
        &owner,
        "PUT",
        &format!("/api/tasks/{a}"),
        Some(serde_json::json!({ "status": "failed" })),
    )
    .await;
    assert_eq!(status, 200, "failed write failed: {body}");

    // The engine runs DETACHED from the status write (a pause may never cost
    // the ticket write it rode in on), so the pause is awaited by POLLING —
    // same contract as await_notification below, not a one-shot read racing
    // the spawned task.
    let mut paused: Option<(bool,)> = None;
    for _ in 0..50 {
        paused = sqlx::query_as("select paused from task_workchains where id = $1::uuid")
            .bind(&chain)
            .fetch_optional(pg)
            .await
            .expect("chain row survives the failed write");
        if paused == Some((true,)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(paused, Some((true,)), "the engine paused the chain");

    let row = await_notification(pg, &f.owner.id, "workchain_paused").await;
    let (title, href) = row.expect("the chain's creator hears about the pause");
    assert!(
        title.starts_with("Workchain paused: Paused train - "),
        "the pause title names the chain and the ticket: {title}"
    );
    assert_eq!(href, format!("/boards/{}/{}", f.board_id, a));

    // A human PATCH resumes the chain — and unpausing auto-advances
    // nothing: the head is still B, exactly as the read endpoint reports.
    let (status, body) = call(
        &state,
        &owner,
        "PATCH",
        &format!("/api/workchains/{chain}"),
        Some(serde_json::json!({ "paused": false })),
    )
    .await;
    assert_eq!(status, 200, "unpause failed: {body}");
    let (status, body) = call(
        &state,
        &owner,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let steps = body["workchains"][0]["steps"].as_array().expect("steps");
    assert_eq!(
        steps[0]["state"], "done",
        "'failed' is terminal for the chain"
    );
    assert_eq!(
        steps[1]["state"], "blocked",
        "the failed pred is not satisfaction: the successor stays blocked \
         (the chain paused; unpausing auto-advances nothing)"
    );
    assert_eq!(
        body["workchains"][0]["paused"],
        Value::Bool(false),
        "the PATCH carried"
    );

    reset(pg, "pause").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn an_archived_step_reads_past_and_the_engine_leaves_it_alone() {
    let state = app_state().await;
    let f = fixture(&state, "archive").await;
    let owner = sid(&state, &f.owner).await;
    let pg = &state.pg;

    let a = ticket(pg, &f.board_id, "Retired mid-chain", "in_progress").await;
    let b = ticket(pg, &f.board_id, "Next up", "in_progress").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Archive read-past").await;
    for task_id in [&a, &b] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    // Archive A (a non-status write): the derived head reads past it, and no
    // engine action fires — archival is not the engine's trigger.
    let (status, body) = call(
        &state,
        &owner,
        "PUT",
        &format!("/api/tasks/{a}"),
        Some(serde_json::json!({ "archived": true })),
    )
    .await;
    assert_eq!(status, 200, "archive write failed: {body}");
    let (status, body) = call(
        &state,
        &owner,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let steps = body["workchains"][0]["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["state"], "archived");
    assert_eq!(
        steps[1]["state"], "ready",
        "the chain reads past the retired step: preds satisfied, servable"
    );

    let rows: Vec<(String,)> = sqlx::query_as(
        "select 1 from notifications where kind in ('workchain_turn', 'workchain_paused')",
    )
    .fetch_all(pg)
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "archival alone files no engine notification"
    );

    reset(pg, "archive").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_wire_draws_then_refuses_cycles_then_unwires() {
    let state = app_state().await;
    let f = fixture(&state, "wires").await;
    let owner = sid(&state, &f.owner).await;
    let pg = &state.pg;

    let a = ticket(pg, &f.board_id, "First", "in_progress").await;
    let b = ticket(pg, &f.board_id, "Second", "in_progress").await;
    let c = ticket(pg, &f.board_id, "Third", "in_progress").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Wire rig").await;
    for task_id in [&a, &b, &c] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }

    // Three step-adds arrive PRE-WIRED a→b→c: consecutive adds keep the
    // v1 linear shape (the canvas rewires from there). Asserting the
    // auto-wire against a real database pins that compat path.
    let n: (i64,) = sqlx::query_as(
        "select count(*)::bigint from task_workchain_edges where workchain_id = $1::uuid",
    )
    .bind(&chain)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(n, (2,), "consecutive step-adds auto-wire the line");

    // A duplicate draw is a quiet ok — the wire's unique index absorbs it.
    // This POST also drives the would_cycle read (the site that broke CI
    // with alias rot) against a live database for the first time.
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/edges"),
        Some(serde_json::json!({ "fromTaskId": a, "toTaskId": b })),
    )
    .await;
    assert_eq!(status, 200, "edge draw failed: {body}");
    let n: (i64,) = sqlx::query_as(
        "select count(*)::bigint from task_workchain_edges where workchain_id = $1::uuid",
    )
    .bind(&chain)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(n, (2,), "the duplicate wire is absorbed, not duplicated");

    // b → a would close a 2-cycle; the router refuses BEFORE any write.
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/edges"),
        Some(serde_json::json!({ "fromTaskId": b, "toTaskId": a })),
    )
    .await;
    assert_eq!(status, 400, "a back-wire must be refused: {body}");
    let n: (i64,) = sqlx::query_as(
        "select count(*)::bigint from task_workchain_edges where workchain_id = $1::uuid",
    )
    .bind(&chain)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(n, (2,), "the refused wire stored nothing");

    // Unwire b→c, then draw a FRESH edge c → a: the insert path (not the
    // dedup) finally writes.
    let (status, body) = call(
        &state,
        &owner,
        "DELETE",
        &format!("/api/workchains/{chain}/edges/{b}/{c}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "edge delete failed: {body}");
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/edges"),
        Some(serde_json::json!({ "fromTaskId": c, "toTaskId": a })),
    )
    .await;
    assert_eq!(status, 200, "fresh edge draw failed: {body}");

    // Now b → c is the long way round (c → a → b): refused. The walk —
    // not a two-hop check — sees through the fresh edge.
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/edges"),
        Some(serde_json::json!({ "fromTaskId": b, "toTaskId": c })),
    )
    .await;
    assert_eq!(status, 400, "the long way round is still a cycle: {body}");

    // The wire list echoes what the canvas draws, in draw order.
    let (status, body) = call(
        &state,
        &owner,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let edges = body["workchains"][0]["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 2, "two wires listed: {edges:?}");
    assert_eq!(edges[0]["fromTaskId"], a);
    assert_eq!(edges[0]["toTaskId"], b);
    assert_eq!(edges[1]["fromTaskId"], c);
    assert_eq!(edges[1]["toTaskId"], a);

    reset(pg, "wires").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn wire_false_lands_a_step_with_no_edges() {
    // The canvas's create-and-connect: the step lands where it was dropped
    // and carries exactly the wire the gesture drew. With the default
    // auto-wire the api also hung it off the chain's TAIL, so a card dragged
    // out of A arrived with a second, invisible predecessor — and derived
    // BLOCKED behind work it was never wired to.
    let state = app_state().await;
    let f = fixture(&state, "unwired").await;
    let owner = sid(&state, &f.owner).await;
    let pg = &state.pg;

    let a = ticket(pg, &f.board_id, "Entry", "in_progress").await;
    let tail = ticket(pg, &f.board_id, "Tail", "in_progress").await;
    let fresh = ticket(pg, &f.board_id, "Dropped on the canvas", "in_progress").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Unwired rig").await;

    let edge_count = async |chain: &str| -> i64 {
        let (n,): (i64,) = sqlx::query_as(
            "select count(*)::bigint from task_workchain_edges where workchain_id = $1::uuid",
        )
        .bind(chain)
        .fetch_one(pg)
        .await
        .unwrap();
        n
    };

    for task_id in [&a, &tail] {
        let (status, body) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200, "step add failed: {body}");
    }
    assert_eq!(
        edge_count(&chain).await,
        1,
        "the default append wires a→tail"
    );

    // wire: false — the step joins the chain, the graph does not change.
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/steps"),
        Some(serde_json::json!({ "taskId": fresh, "wire": false })),
    )
    .await;
    assert_eq!(status, 200, "unwired step add failed: {body}");
    assert_eq!(
        edge_count(&chain).await,
        1,
        "wire: false draws nothing — no tail edge volunteered"
    );
    assert_eq!(
        chain_order(pg, &chain).await,
        vec![a.clone(), tail.clone(), fresh.clone()],
        "the step is still appended, it is only unwired"
    );

    // …and with no predecessors it derives HEAD, not blocked behind the tail.
    let (status, body) = call(
        &state,
        &owner,
        "GET",
        &format!("/api/boards/{}/workchains", f.board_id),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let steps = body["workchains"][0]["steps"].as_array().expect("steps");
    let dropped = steps
        .iter()
        .find(|s| s["taskId"] == fresh.as_str())
        .expect("the dropped step is listed");
    assert_eq!(dropped["state"], "head", "an unwired step is its own head");

    // The caller then draws the ONE wire it meant: a → fresh.
    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/edges"),
        Some(serde_json::json!({ "fromTaskId": a, "toTaskId": fresh })),
    )
    .await;
    assert_eq!(status, 200, "edge draw failed: {body}");
    assert_eq!(edge_count(&chain).await, 2, "a fan-out off a, nothing else");

    reset(pg, "unwired").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_refused_wedge_leaves_every_position_alone() {
    // The `after` step must belong to this chain, and the 400 that says so
    // must not have moved anything: the position shift used to run on the
    // POOL rather than the transaction, so it committed on its own and every
    // later step was pushed up by one before the refusal.
    let state = app_state().await;
    let f = fixture(&state, "wedge").await;
    let owner = sid(&state, &f.owner).await;
    let pg = &state.pg;

    let a = ticket(pg, &f.board_id, "First", "in_progress").await;
    let b = ticket(pg, &f.board_id, "Second", "in_progress").await;
    let outsider = ticket(pg, &f.board_id, "Not in this chain", "in_progress").await;
    let latecomer = ticket(pg, &f.board_id, "Wedge me", "in_progress").await;
    let chain = create_chain(&state, &owner, &f.board_id, "Wedge rig").await;
    for task_id in [&a, &b] {
        let (status, _) = call(
            &state,
            &owner,
            "POST",
            &format!("/api/workchains/{chain}/steps"),
            Some(serde_json::json!({ "taskId": task_id })),
        )
        .await;
        assert_eq!(status, 200);
    }
    let positions_before: Vec<(String, i32)> = sqlx::query_as(
        "select task_id::text, position from task_workchain_steps \
         where workchain_id = $1::uuid order by position",
    )
    .bind(&chain)
    .fetch_all(pg)
    .await
    .unwrap();

    let (status, body) = call(
        &state,
        &owner,
        "POST",
        &format!("/api/workchains/{chain}/steps"),
        Some(serde_json::json!({ "taskId": latecomer, "after": outsider })),
    )
    .await;
    assert_eq!(status, 400, "a foreign anchor is refused: {body}");

    let positions_after: Vec<(String, i32)> = sqlx::query_as(
        "select task_id::text, position from task_workchain_steps \
         where workchain_id = $1::uuid order by position",
    )
    .bind(&chain)
    .fetch_all(pg)
    .await
    .unwrap();
    assert_eq!(
        positions_before, positions_after,
        "the refused wedge rolled its position shift back"
    );

    reset(pg, "wedge").await;
}

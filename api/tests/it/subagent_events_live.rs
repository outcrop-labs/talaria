// Live-DB proof of the subagent door (cargo test -- --ignored): POST
// /api/agents/subagent-events, the inbound half of the talaria-events
// plugin's subagent_start/subagent_stop hooks, and the `maxChildren` cap the
// render turns into `delegation.max_concurrent_children`.
//
// None of it is provable without Postgres. What a delegation reads as in the
// chat is ONE chip per child that changes in place — "reviewer · check the
// binds", running, then done with its summary — and whether that holds is
// decided by the chip merge's SQL against the real `chips` jsonb and by
// `speaking_target`'s interval arithmetic picking the turn to land on. The
// cap is a column the route writes and the def wire reads back. House rule:
// #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it subagent_events_live:: -- --ignored

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_agent_managers::add_manager;
use talaria_api::conversations::{create_conversation, insert_streaming_assistant};
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

use crate::support::{
    call_json, fabricate_user, live_app_state, minted_member_with_email, sweep_user_rows,
};

const PREFIX: &str = "subagent-events-live";

/// Every test owns its OWN tag and sweeps only its own rows: these cases run
/// concurrently, and a shared pattern would have one test deleting the agent
/// — or the person whose conversation carries the turn — another is mid-way
/// through asserting against. The def goes first (agent_keys and
/// agent_managers cascade off it), then the person (conversations, and so
/// the messages row the chips live on, cascade off them).
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, PREFIX, tag).await;
}

/// An agent_defs row that names no real provider, so nothing dispatches it
/// and the PATCH's roll (managed-only) enqueues nothing. Both `slug` and
/// `model` are UNIQUE, so both carry a uuid: a rerun cannot collide with a
/// previous run's leftovers. Returns (def id, model).
async fn def_row(pg: &PgPool, tag: &str) -> (String, String) {
    let slug = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let model = format!("{slug}-model");
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'personal', $2, 'Subagent Events Live') returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, model)
}

/// A fabricated agent that is MID-TURN: its own minted credential (the exact
/// thing a container presents) plus a streaming assistant row on a
/// conversation whose `agent_model` is this agent — which is what makes that
/// row the chip's landing target. `speaking_target` asks for role
/// 'assistant', status 'streaming' and a `streamed_at` inside the last 15
/// minutes; the column defaults to now(), so a freshly inserted turn is
/// live. Returns (credential, message id).
async fn agent_mid_turn(state: &AppState, tag: &str) -> (String, String) {
    let pg = &state.pg;
    let (def_id, model) = def_row(pg, tag).await;
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(pg, &sb, &def_id)
        .await
        .expect("agent key mints");
    let user = fabricate_user(pg, PREFIX, tag).await;
    let conversation = create_conversation(pg, &user, &model, "Subagent events", "chat", None)
        .await
        .unwrap();
    // The person's ask, then the turn the agent is still writing — the shape
    // a delegation actually happens inside.
    sqlx::query(
        "insert into messages (conversation_id, seq, role, status, content) \
         values ($1::uuid, 0, 'user', 'complete', 'delegate the review')",
    )
    .bind(&conversation)
    .execute(pg)
    .await
    .unwrap();
    let turn = insert_streaming_assistant(pg, &conversation, 1, &json!({}))
        .await
        .unwrap();
    (key, turn)
}

/// One POST through the REAL router with the agent's own credential. The
/// door is agent-authenticated, never session-authenticated (the same door
/// as tool-events), and `support::call_json` only knows how to send a
/// cookie — hence the hand-built request. `key: None` is the unkeyed caller.
async fn post_event(state: &AppState, key: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/agents/subagent-events")
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("x-api-key", key);
    }
    let res = talaria_api::routes::router(state.clone())
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The chips as STORED on the turn — the landed truth, read back from
/// Postgres rather than from the route's answer.
async fn chips_on(pg: &PgPool, message: &str) -> Vec<Value> {
    let (chips,): (Value,) = sqlx::query_as("select chips from messages where id = $1::uuid")
        .bind(message)
        .fetch_one(pg)
        .await
        .unwrap();
    chips.as_array().cloned().unwrap_or_default()
}

/// The one chip a child owns. Identity is the child's session id, which is
/// the whole point: a start and its stop are the same chip.
fn chip_for<'a>(chips: &'a [Value], child: &str) -> &'a Value {
    let id = Value::String(format!("subagent:{child}"));
    chips
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no chip for {child}: {chips:?}"))
}

async fn max_children(pg: &PgPool, def_id: &str) -> i32 {
    let (n,): (i32,) = sqlx::query_as("select max_children from agent_defs where id = $1::uuid")
        .bind(def_id)
        .fetch_one(pg)
        .await
        .unwrap();
    n
}

/// The delegation as a reader sees it: one chip that starts running and ends
/// done, carrying the summary and the tools the child used. A second chip
/// for the stop is the failure this test exists to prevent — the chat would
/// show the same child twice, once forever running.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_start_then_a_stop_is_one_chip_that_turns_done() {
    let state = live_app_state().await;
    let tag = "pair";
    cleanup(&state.pg, tag).await;
    let (key, turn) = agent_mid_turn(&state, tag).await;
    let child = format!("child-{}", uuid::Uuid::new_v4());
    // Longer than the label's clip, with a distinctive tail: the assertion
    // proves the clip rather than merely the prefix.
    let goal = format!("{}-tail-past-the-clip", "delegate the review ".repeat(5));

    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({
            "kind": "start",
            "parentSessionId": "parent-session",
            "parentTurnId": "parent-turn",
            "childSessionId": child,
            "childRole": "reviewer",
            "childGoal": goal,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(
        body["landed"], true,
        "the agent is mid-turn, so the chip has a turn to land on: {body}"
    );
    let chips = chips_on(&state.pg, &turn).await;
    assert_eq!(chips.len(), 1, "one child, one chip: {chips:?}");
    let chip = chip_for(&chips, &child);
    assert_eq!(chip["kind"], "subagent");
    assert_eq!(chip["status"], "running", "a start is work in flight");
    assert_eq!(
        chip["label"],
        format!("reviewer · {}", &goal[..80]),
        "the label is the role and the goal clipped to 80"
    );
    assert!(
        !chip["label"]
            .as_str()
            .unwrap_or_default()
            .contains("tail-past-the-clip"),
        "the clipped tail must not ride into the label: {chip}"
    );
    assert_eq!(chip["goal"], goal, "the full goal stays on the chip");

    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({
            "kind": "stop",
            "parentSessionId": "parent-session",
            "parentTurnId": "parent-turn",
            "childSessionId": child,
            "childRole": "reviewer",
            "childGoal": goal,
            "childSummary": "two bind casts, both fixed",
            "childStatus": "done",
            "tools": ["read_file", "grep", "write_file"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["landed"], true);
    let chips = chips_on(&state.pg, &turn).await;
    assert_eq!(
        chips.len(),
        1,
        "the stop overwrites its start IN PLACE, never a second chip: {chips:?}"
    );
    let chip = chip_for(&chips, &child);
    assert_eq!(chip["status"], "done");
    assert_eq!(chip["summary"], "two bind casts, both fixed");
    assert_eq!(
        chip["tools"],
        json!(["read_file", "grep", "write_file"]),
        "the tools the child used are the chip's detail"
    );

    // A child that failed reads error, not done: the status is the child's
    // own, and a failure that printed as "done" would be the one thing a
    // reader must never be told.
    let failed = format!("child-{}", uuid::Uuid::new_v4());
    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({
            "kind": "start",
            "parentSessionId": "parent-session",
            "childSessionId": failed,
            "childRole": "tester",
            "childGoal": "run the suite",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({
            "kind": "stop",
            "parentSessionId": "parent-session",
            "childSessionId": failed,
            "childRole": "tester",
            "childGoal": "run the suite",
            "childSummary": "the suite never linked",
            "childStatus": "error",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let chips = chips_on(&state.pg, &turn).await;
    assert_eq!(chips.len(), 2, "two children, two chips: {chips:?}");
    assert_eq!(chip_for(&chips, &failed)["status"], "error");
    assert_eq!(
        chip_for(&chips, &child)["status"],
        "done",
        "the first child's chip is untouched by the second's stop"
    );

    cleanup(&state.pg, tag).await;
}

/// A stop whose start never arrived still lands. The door is not a state
/// machine over starts: a plugin that was rolled mid-delegation, or a queue
/// that dropped the start, must not also cost us the ONE event that says the
/// work finished. A 404 here would turn a lost start into a lost result.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_stop_for_a_child_we_never_saw_start_still_lands() {
    let state = live_app_state().await;
    let tag = "orphan";
    cleanup(&state.pg, tag).await;
    let (key, turn) = agent_mid_turn(&state, tag).await;
    let child = format!("child-{}", uuid::Uuid::new_v4());

    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({
            "kind": "stop",
            "parentSessionId": "parent-session",
            "childSessionId": child,
            "childRole": "researcher",
            "childGoal": "read the migration notes",
            "childSummary": "the snapshot is regenerated, not hand-edited",
            "childStatus": "done",
            "tools": ["read_file"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["landed"], true);
    let chips = chips_on(&state.pg, &turn).await;
    assert_eq!(
        chips.len(),
        1,
        "the orphaned stop is still a chip: {chips:?}"
    );
    let chip = chip_for(&chips, &child);
    assert_eq!(chip["status"], "done");
    assert_eq!(
        chip["summary"], "the snapshot is regenerated, not hand-edited",
        "the result is what the reader came for"
    );

    cleanup(&state.pg, tag).await;
}

/// The door's refusals, and that a refused event writes nothing. The kind is
/// the whole shape of the body, the child's session id is the chip's
/// identity, and the credential is the only thing that names the parent — a
/// caller without one has no agent whose turn to land on.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_door_refuses_a_bogus_kind_and_an_unkeyed_caller() {
    let state = live_app_state().await;
    let tag = "guard";
    cleanup(&state.pg, tag).await;
    let (key, turn) = agent_mid_turn(&state, tag).await;
    let child = format!("child-{}", uuid::Uuid::new_v4());

    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({ "kind": "bogus", "childSessionId": child, "childRole": "reviewer" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = post_event(
        &state,
        Some(&key),
        json!({ "kind": "start", "childRole": "reviewer", "childGoal": "nameless" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a chip with no identity cannot be overwritten by its own stop: {body}"
    );

    let (status, body) = post_event(
        &state,
        None,
        json!({ "kind": "start", "childSessionId": child, "childRole": "reviewer" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    assert!(
        chips_on(&state.pg, &turn).await.is_empty(),
        "a refused event lands nothing on the turn"
    );

    cleanup(&state.pg, tag).await;
}

/// The cap: PATCH /api/fleet/defs/{id} takes `maxChildren` 0..16, the def
/// keeps it, and the def wire carries it back — the number the render emits
/// as `delegation.max_concurrent_children`, so a fleet operator can hold a
/// delegating agent to a budget. Over the cap is a 400 that changes nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn max_children_is_a_capped_number_the_def_keeps() {
    let state = live_app_state().await;
    let tag = "cap";
    cleanup(&state.pg, tag).await;
    let (def_id, _model) = def_row(&state.pg, tag).await;
    let email = format!("{}@subagent-events-live.invalid", uuid::Uuid::new_v4());
    let (user, cookie) = minted_member_with_email(&state, &format!("{PREFIX}:{tag}"), &email).await;
    // The write gate is require_agent_manager, and being NAMED is the grant
    // — this member needs no org-wide permission to hold their own agent to
    // a child budget.
    add_manager(&state.pg, &def_id, &user, Some(&user))
        .await
        .unwrap();
    let url = format!("/api/fleet/defs/{def_id}");

    assert_eq!(
        max_children(&state.pg, &def_id).await,
        3,
        "the column's default: three children until someone says otherwise"
    );

    let (status, body) = call_json(
        &state,
        "PATCH",
        &url,
        Some(&cookie),
        Some(json!({ "maxChildren": 17 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        max_children(&state.pg, &def_id).await,
        3,
        "a refused patch leaves the cap standing"
    );

    let (status, body) = call_json(
        &state,
        "PATCH",
        &url,
        Some(&cookie),
        Some(json!({ "maxChildren": 4 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(max_children(&state.pg, &def_id).await, 4);

    // And the roster reads it from the def wire, not from a second source.
    let (status, body) = call_json(&state, "GET", "/api/fleet/defs", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let def = body["defs"]
        .as_array()
        .expect("the fleet wire carries defs")
        .iter()
        .find(|d| d["id"] == Value::String(def_id.clone()))
        .expect("a manager reads the agent they manage");
    assert_eq!(def["maxChildren"], 4);

    cleanup(&state.pg, tag).await;
}

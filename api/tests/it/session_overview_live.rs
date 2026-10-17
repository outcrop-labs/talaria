// Live-DB proof for GET /api/conversations/{id}/session — the HERMES-FREE
// half of it.
//
// What a test box can prove about this route is everything up to the container
// door: the access gate (a thread you cannot see is a 403, and it is a 403
// whether the thread exists or not), the pinned question (an agent whose def
// does not pin sessions has no Hermes session to describe), and the
// unreachable answer (a def that DOES pin, on a model no fleet manifest names,
// cannot be asked — and that is a 200 saying so, not an error). Each of those
// is a stack of WHERE clauses over users, agent_defs, conversations and
// messages that only a real database answers.
//
// What it cannot prove is the numbers: `compacted`, `since` and `messageCount`
// come out of a running Hermes container holding a real session, and a mock of
// that would only prove the mock. The parser those numbers go through is unit
// tested in talaria-gateway (fleet_chat's `parse_session_rows`).
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it session_overview_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::conversations::{create_conversation, insert_user_message};
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const PREFIX: &str = "session-overview-live";

/// A user row and a session minted the way the auth routes mint one. Every
/// test passes its OWN tag: the sweep below deletes by that tag, so a failed
/// run's leftovers can only ever shadow the test that made them.
async fn signed_in(state: &AppState, tag: &str, role: &str) -> (SessionUser, String) {
    let who = uuid::Uuid::new_v4();
    let sub = format!("{PREFIX}:{tag}:{who}");
    let email = format!("{tag}-{who}@{PREFIX}.invalid");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, role) values ($1, $2, $3) returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .bind(role)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let user = SessionUser {
        id,
        sub,
        email: Some(email),
        name: None,
        picture: None,
        role: role.to_string(),
        provider: "password".into(),
    };
    let sid = create_session(state, &user).await.expect("session mints");
    let cookie = format!("talaria_session={sid}");
    (user, cookie)
}

/// Delete what this tag's previous run left behind, at the START of a test.
/// Users cascade to their conversations and conversations to their messages,
/// so the agent rows are the only second sweep this suite needs.
async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
}

/// An agent_defs row that names no real provider and runs in no container.
/// `pinned` is the switch this route branches on; the model is deliberately a
/// name NO fleet manifest carries, which is what makes the pinned case
/// unreachable rather than flaky. Returns (defs id, model); slug and model are
/// one spelling so the sweep is one pattern.
async fn agent_def(pg: &PgPool, tag: &str, owner: &str, pinned: bool) -> (String, String) {
    let model = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs \
             (slug, department, model, display_name, owner_user_id, pinned_sessions) \
         values ($1, 'personal', $2, 'Session Overview Live Agent', $3::uuid, $4) \
         returning id::text",
    )
    .bind(&model)
    .bind(&model)
    .bind(owner)
    .bind(pinned)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, model)
}

/// A one-turn chat with this agent. One USER row and no assistant row is a
/// FRESH history — `pinned_history_fits` says yes to it, so the pinned tests
/// reach the container door instead of stopping at the history rule.
async fn chat(pg: &PgPool, owner: &str, model: &str) -> String {
    let conv = create_conversation(pg, owner, model, "live session overview", "chat", None)
        .await
        .unwrap();
    insert_user_message(
        pg,
        &conv,
        0,
        "what do you remember",
        &json!([]),
        Some(owner),
        &json!({}),
    )
    .await
    .unwrap();
    conv
}

/// The "this does not describe your thread" answer, whole — asserted as one
/// shape, because the contract is that every "no" looks identical.
fn assert_not_applicable(body: &Value) {
    assert_eq!(body["applies"], json!(false), "{body}");
    assert_eq!(body["reachable"], json!(false), "{body}");
    assert_eq!(body["compacted"], json!(0), "{body}");
    assert_eq!(body["since"], Value::Null, "{body}");
    assert_eq!(body["sessionId"], Value::Null, "{body}");
    assert_eq!(body["messageCount"], Value::Null, "{body}");
}

/// The access gate, and it is the house 403: a thread somebody else owns and a
/// thread that does not exist answer the same way, so the failure cannot be
/// read as "that conversation is real, just not yours".
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_thread_the_caller_cannot_see_is_a_403() {
    let state = live_app_state().await;
    sweep(&state.pg, "see").await;
    let (owner, _) = signed_in(&state, "see", "member").await;
    let (_, stranger) = signed_in(&state, "see", "member").await;
    let (_, model) = agent_def(&state.pg, "see", &owner.id, false).await;
    let conv = chat(&state.pg, &owner.id, &model).await;

    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}/session"),
        Some(&stranger),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], json!("forbidden"));

    // A conversation nobody ever created is the SAME answer.
    let nowhere = uuid::Uuid::new_v4();
    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{nowhere}/session"),
        Some(&stranger),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // And no session at all is the session gate's 401, not this route's 403.
    let (status, _) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}/session"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// An agent that is not pinned keeps no Hermes session: Talaria holds that
/// transcript and nothing has been compacted away. A 200 saying "does not
/// apply", because a drawer asks this of every thread it opens.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_unpinned_agents_thread_does_not_apply() {
    let state = live_app_state().await;
    sweep(&state.pg, "unpinned").await;
    let (owner, cookie) = signed_in(&state, "unpinned", "member").await;
    let (_, model) = agent_def(&state.pg, "unpinned", &owner.id, false).await;
    let conv = chat(&state.pg, &owner.id, &model).await;

    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}/session"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_not_applicable(&body);
}

/// Pinned, with a fresh history, on a model no manifest names — the agent
/// cannot be asked. That is `reachable: false` and a 200: the container being
/// down is a fact about the fleet, not a failure of the person's request, and
/// the route must not turn it into a console error on an ordinary chat.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_pinned_agent_with_no_manifest_row_is_unreachable() {
    let state = live_app_state().await;
    sweep(&state.pg, "unreachable").await;
    let (owner, cookie) = signed_in(&state, "unreachable", "member").await;
    let (_, model) = agent_def(&state.pg, "unreachable", &owner.id, true).await;
    let conv = chat(&state.pg, &owner.id, &model).await;

    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}/session"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_not_applicable(&body);
}

/// The same pinned def, on a conversation whose assistant rows carry no
/// hermes_run_id — the legacy full-history transcript Hermes never saw. The
/// pinned flip does not retroactively move a thread, and the overview must
/// not claim to describe one.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_pinned_agents_legacy_thread_does_not_apply() {
    let state = live_app_state().await;
    sweep(&state.pg, "legacy").await;
    let (owner, cookie) = signed_in(&state, "legacy", "member").await;
    let (_, model) = agent_def(&state.pg, "legacy", &owner.id, true).await;
    let conv = chat(&state.pg, &owner.id, &model).await;
    // An assistant row with no run id: answered through the full-history path.
    sqlx::query(
        "insert into messages (conversation_id, seq, role, content, status) \
         values ($1::uuid, 1, 'assistant', 'answered the old way', 'complete')",
    )
    .bind(&conv)
    .execute(&state.pg)
    .await
    .unwrap();

    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}/session"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_not_applicable(&body);
}

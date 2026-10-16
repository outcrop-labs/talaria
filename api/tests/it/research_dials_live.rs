// Live-DB proofs for the research DIALS at the edge — POST /api/research's own
// door, driven through the real router with a real session.
//
// WHAT THIS FILE IS ABOUT: the dials (effort/breadth/sources, 1..=3 each) and
// the conversation a run can be pointed at are the two things a caller hands
// in from outside, and both have to be settled BEFORE the run costs anything.
// A dial off the scale must come back as a 400 that names the dial — not as a
// clamped run that quietly researched at a depth nobody asked for, and not as
// a 500 from the engine. A conversation the caller cannot see must come back
// as a 404 from the door — not reach `start_research` and get written onto a
// run row. Those orderings are not visible in a unit test of the parser: they
// are a property of where the checks sit in the handler, against the real
// predicates and the real schema. Hence live.
//
// WHAT IS NOT HERE: the positive end-to-end — a run started with a
// conversation the caller owns, landing `research_runs.conversation_id` and
// `dials` and reading back on GET. `start_research` refuses up front unless
// `plan_search` finds a proven search path (a registered native searcher, or a
// connected backend), and that is workspace-global configuration: a test
// cannot fake it without rewriting settings every other session on this
// database shares, and once it is faked the run really runs — real model
// calls, real money. The existing research live module (`research_live.rs`)
// fabricates its rows with SQL for exactly this reason and never goes near the
// planner. So the gate itself is what gets pinned below: the run is refused
// with the exported sentence, which proves the dials and the conversation were
// read first.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it research_dials_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::conversations::create_conversation;
use talaria_api::research::{NO_SEARCH_REASON, plan_search};
use talaria_api::state::AppState;
use talaria_harness_defs::defs::research::ResearchDepth;

use crate::support::{call_json, live_app_state, minted_member_with_email, sweep_user_rows};

const PREFIX: &str = "research-dials-live";

/// The agent the run would be pinned to. A member with no `user_agent_access`
/// rows may use any agent (access is grant-NARROWING), so a fabricated id
/// passes the gate without putting a fleet row on this database.
const AGENT: &str = "research-dials-live-agent";

/// A member who may start research, with a session cookie. The permission is
/// INSERTED rather than inherited: `research.run` is a member default today,
/// and a test that leaned on that would start failing the day an admin tuned
/// it off for the org — on a failure that says nothing about the dials.
async fn researcher(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@{PREFIX}.invalid", uuid::Uuid::new_v4());
    let (id, cookie) = minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await;
    sqlx::query(
        "insert into user_permissions (user_id, perm, allowed) \
         values ($1::uuid, 'research.run', true) \
         on conflict (user_id, perm) do update set allowed = true",
    )
    .bind(&id)
    .execute(&state.pg)
    .await
    .unwrap();
    (id, cookie)
}

/// A question no other row carries: the route answers 409 for a question
/// already queued or running, and a duplicate would shadow every assertion
/// below.
fn question() -> String {
    format!("what should the dials do about {}?", uuid::Uuid::new_v4())
}

/// A start body with one extra member — the dial or the conversation under
/// test. Built as a map rather than `json!` so the key can be a variable.
fn start_body(extra: &[(&str, Value)]) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("question".into(), json!(question()));
    body.insert("mode".into(), json!("recon"));
    body.insert("agentModel".into(), json!(AGENT));
    for (key, value) in extra {
        body.insert((*key).to_string(), value.clone());
    }
    Value::Object(body)
}

/// How many runs this owner has. The dial refusals below are all "before
/// anything happens" claims, and the only way to say that is to look.
async fn runs_owned(state: &AppState, owner: &str) -> i64 {
    let (n,): (i64,) =
        sqlx::query_as("select count(*) from research_runs where owner_user_id = $1::uuid")
            .bind(owner)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    n
}

/// A dial outside 1..=3 — too big, too small, or not a number at all — is a
/// 400 that NAMES THE DIAL. Three knobs ride on one call, so "out of range"
/// would leave the caller guessing which one to move; and the refusal lands
/// before a row exists, which is the other half of the claim.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_dial_off_the_scale_is_refused_by_name_and_starts_nothing() {
    let state = live_app_state().await;
    let (me, cookie) = researcher(&state, "refusals").await;

    for (field, value) in [
        ("effort", json!(4)),
        ("breadth", json!(0)),
        ("sources", json!("x")),
    ] {
        let (status, body) = call_json(
            &state,
            "POST",
            "/api/research",
            Some(&cookie),
            Some(start_body(&[(field, value.clone())])),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field} = {value} must be refused, got {status}: {body}"
        );
        let said = body["error"].as_str().unwrap_or_default();
        assert!(
            said.contains(field),
            "the refusal must name the dial it is about — {field} = {value} said {said:?}"
        );
    }

    assert_eq!(
        runs_owned(&state, &me).await,
        0,
        "a refused dial is refused at the door: no run row, no enqueue"
    );

    sweep_user_rows(&state.pg, PREFIX, "refusals").await;
}

/// POINTING A RUN AT SOMEBODY ELSE'S CONVERSATION. `conversationId` says
/// "discuss and report in here", so the caller must be able to see it — read
/// through the same chat-door predicate every other surface asks, answered as
/// a 404 (not a 403): a conversation you cannot see is one that, for you, is
/// not there.
///
/// THE ORDERING IS THE POINT, and it is why this test carries valid dials it
/// does not assert on: a 404 here means the door settled both the dials and
/// the conversation before `start_research` was ever called. Nothing is
/// started, on any workspace, which is also why this proof is the one that
/// runs everywhere.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_conversation_the_caller_cannot_see_is_not_found() {
    let state = live_app_state().await;
    let (me, cookie) = researcher(&state, "stranger").await;
    let (owner, _) = researcher(&state, "stranger-owner").await;
    let theirs = create_conversation(
        &state.pg,
        &owner,
        AGENT,
        "somebody else's plan",
        "plan",
        None,
    )
    .await
    .unwrap();

    let (status, body) = call_json(
        &state,
        "POST",
        "/api/research",
        Some(&cookie),
        Some(start_body(&[
            ("conversationId", json!(theirs)),
            ("effort", json!(1)),
            ("breadth", json!(2)),
            ("sources", json!(3)),
        ])),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a conversation outside the caller's reach is not there to report into, got {status}: {body}"
    );
    let said = body["error"].as_str().unwrap_or_default();
    assert!(
        said.to_lowercase().contains("conversation not found"),
        "the 404 says which door closed — got {said:?}"
    );
    for dial in ["effort", "breadth", "sources"] {
        assert!(
            !said.contains(dial),
            "the dials were in range; the refusal is the conversation's — got {said:?}"
        );
    }
    assert_eq!(
        runs_owned(&state, &me).await,
        0,
        "the door answered before the engine: no run row"
    );

    sweep_user_rows(&state.pg, PREFIX, "stranger").await;
    sweep_user_rows(&state.pg, PREFIX, "stranger-owner").await;
}

/// THE DIALS PARSE BEFORE THE SEARCH GATE. A well-formed start on a workspace
/// that cannot search yet comes back as the exported sentence — the one the
/// route, the MCP tool and the run's own `begin` step all say — which can only
/// happen if the dials and the conversation were read, accepted, and handed to
/// `start_research`. A 500 here, or a 400 naming a dial that is plainly in
/// range, is the regression this exists to catch.
///
/// The conversation is the caller's OWN plan, so this is also the positive
/// direction of the door: an accessible conversation gets through it.
///
/// A workspace WITH a search path is left alone. There the same call would
/// really start a run — real model calls — and a test does not get to spend an
/// org's money to watch a gate it has already been told is open.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn valid_dials_and_an_own_conversation_reach_the_search_gate() {
    let state = live_app_state().await;
    if plan_search(&state.pg, ResearchDepth::Recon).await.is_some() {
        eprintln!(
            "skipped: this workspace can search, so a well-formed start would run for real \
             — the gate's refusal is only observable where there is no search path"
        );
        return;
    }
    let (me, cookie) = researcher(&state, "gate").await;
    let mine = create_conversation(&state.pg, &me, AGENT, "my own plan", "plan", None)
        .await
        .unwrap();

    let (status, body) = call_json(
        &state,
        "POST",
        "/api/research",
        Some(&cookie),
        Some(start_body(&[
            ("conversationId", json!(mine)),
            ("effort", json!(3)),
            ("breadth", json!(1)),
            ("sources", json!(2)),
        ])),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the gate refuses the run, it does not fail — got {status}: {body}"
    );
    assert_eq!(
        body["error"].as_str().unwrap_or_default(),
        NO_SEARCH_REASON,
        "the one sentence, phrased for the person who can fix it"
    );
    assert_eq!(
        runs_owned(&state, &me).await,
        0,
        "refused at the gate: nothing queued"
    );

    sweep_user_rows(&state.pg, PREFIX, "gate").await;
}

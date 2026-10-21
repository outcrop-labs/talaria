// Live-DB + live-Redis proof for scheduling an agent from the conversation you
// are already having with it, and for the turn a cron run posts back into that
// same thread.
//
// Two doors, two credentials, and that is the whole point of proving them
// here. POST /api/conversations/{id}/schedule is a PERSON asking: a session
// cookie, the read rules, and then the agent's manager gate — three WHERE
// clauses stacked, each of which a mock would happily agree with. POST
// /api/agents/cron-output is the AGENT reporting: its own `tak_` credential,
// a task id parsed back into the job that was scheduled, and a marker that
// has to make a replayed delivery land once rather than twice. Only a real
// database answers any of it.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it conversation_schedules_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::conversations::{create_conversation, insert_user_message};
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, wire_boot_seams};

const PREFIX: &str = "conv-sched-live";

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
/// Users cascade to their conversations, and conversations cascade to their
/// messages and to the schedules hanging off them, so the agent rows are the
/// only second sweep this suite needs.
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

/// An agent_defs row that names no real provider and runs in no container:
/// `managed` defaults false, which is exactly the shape the schedule route's
/// resolve step has to refuse in a sentence instead of a 500. `owner` is who
/// manages it — None for an agent nobody but an admin manages. Returns
/// (defs id, model); slug and model are one spelling so the sweep is one
/// pattern.
async fn agent_def(pg: &PgPool, tag: &str, owner: Option<&str>) -> (String, String) {
    let model = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, owner_user_id) \
         values ($1, 'personal', $2, 'Schedules Live Agent', $3::uuid) returning id::text",
    )
    .bind(&model)
    .bind(&model)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, model)
}

/// The agent's own credential — the one a container presents, and the only
/// thing that can make `require_agent` resolve a model.
async fn agent_key(state: &AppState, def_id: &str) -> String {
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, def_id)
        .await
        .expect("agent key mints")
}

/// A one-turn chat with this agent: the user's line at seq 0, so "the last
/// message" means something once the cron turn lands behind it.
async fn chat(pg: &PgPool, owner: &str, model: &str) -> String {
    let conv = create_conversation(pg, owner, model, "live schedule", "chat", None)
        .await
        .unwrap();
    insert_user_message(
        pg,
        &conv,
        0,
        "do this nightly",
        &json!([]),
        Some(owner),
        &json!({}),
    )
    .await
    .unwrap();
    conv
}

/// A job id in Hermes' grammar (lowercase hex), which is what the task id the
/// plugin reports is parsed back into.
fn job_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// The row the create route writes, written by hand — the cron-output half is
/// reachable without a container, and a container is the one thing a test box
/// does not have.
async fn schedule_row(pg: &PgPool, model: &str, job: &str, conv: &str, by: &str) {
    sqlx::query(
        "insert into conversation_schedules \
             (agent_model, job_id, conversation_id, name, schedule, created_by) \
         values ($1, $2, $3::uuid, 'nightly digest', '0 9 * * *', $4::uuid)",
    )
    .bind(model)
    .bind(job)
    .bind(conv)
    .bind(by)
    .execute(pg)
    .await
    .unwrap();
}

/// One request through the REAL router with an agent's own key. `call_json`
/// speaks cookies only, and this door never takes one.
async fn call_as_agent(state: &AppState, key: &str, body: Value) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/agents/cron-output")
        .header("x-api-key", key)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = routes::router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// The thread's newest turn: (id, role, content, status, marker).
async fn last_turn(pg: &PgPool, conv: &str) -> (String, String, String, String, Option<String>) {
    sqlx::query_as(
        "select id::text, role, content, status, metadata->>'marker' from messages \
         where conversation_id = $1::uuid order by seq desc limit 1",
    )
    .bind(conv)
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn schedule_count(pg: &PgPool, conv: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from conversation_schedules where conversation_id = $1::uuid",
    )
    .bind(conv)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// The route's notify is detached — it must never cost the turn — so the
/// assertion POLLS, bounded, with the query it wants to run on every attempt.
async fn await_agent_reply(pg: &PgPool, user: &str) -> bool {
    for _ in 0..50 {
        let row: Option<i32> = sqlx::query_scalar(
            "select 1 from notifications \
             where user_id = $1::uuid and kind = 'agent-reply' and read_at is null limit 1",
        )
        .bind(user)
        .fetch_optional(pg)
        .await
        .unwrap();
        if row.is_some() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

// ── The person's door: POST /api/conversations/{id}/schedule ────────────────

/// The read rules first, and they answer 404 rather than 403: a thread you
/// cannot see must not be confirmed to exist by the way scheduling it fails.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_thread_the_caller_cannot_see_is_a_404() {
    let state = live_app_state().await;
    sweep(&state.pg, "see").await;
    let (owner, _) = signed_in(&state, "see", "member").await;
    let (_, stranger) = signed_in(&state, "see", "member").await;
    let (_, model) = agent_def(&state.pg, "see", Some(&owner.id)).await;
    let conv = chat(&state.pg, &owner.id, &model).await;

    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/schedule"),
        Some(&stranger),
        Some(json!({ "name": "nightly", "schedule": "0 9 * * *", "prompt": "summarize" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], json!("conversation not found"));

    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/schedule"),
        None,
        Some(json!({ "name": "nightly", "schedule": "0 9 * * *", "prompt": "summarize" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The field bounds are the fleet cron form's, to the character: a name past
/// 80 is refused before anything reaches a container.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_overlong_name_is_a_400() {
    let state = live_app_state().await;
    sweep(&state.pg, "name").await;
    // An admin manages every agent, so this test fails on the body and
    // nothing else.
    let (admin, cookie) = signed_in(&state, "name", "admin").await;
    let (_, model) = agent_def(&state.pg, "name", None).await;
    let conv = chat(&state.pg, &admin.id, &model).await;

    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/schedule"),
        Some(&cookie),
        Some(json!({ "name": "x".repeat(81), "schedule": "0 9 * * *", "prompt": "summarize" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(schedule_count(&state.pg, &conv).await, 0);
}

/// Being able to READ a thread is not being able to put work on the agent's
/// clock: the gate is the agent's manager list, keyed by model.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_member_who_does_not_manage_the_agent_is_a_403() {
    let state = live_app_state().await;
    sweep(&state.pg, "mgr").await;
    let (chatter, cookie) = signed_in(&state, "mgr", "member").await;
    let (boss, _) = signed_in(&state, "mgr", "member").await;
    // The agent belongs to somebody else — the chatter owns the thread and
    // can read every word of it, which is exactly the shape that must still
    // be refused.
    let (_, model) = agent_def(&state.pg, "mgr", Some(&boss.id)).await;
    let conv = chat(&state.pg, &chatter.id, &model).await;

    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/schedule"),
        Some(&cookie),
        Some(json!({ "name": "nightly", "schedule": "0 9 * * *", "prompt": "summarize" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let said = body["error"].as_str().unwrap_or_default();
    assert!(
        said.contains("managers"),
        "the refusal names the gate, got {said:?}"
    );
    assert_eq!(schedule_count(&state.pg, &conv).await, 0);
}

/// THE FAILURE THAT WOULD OTHERWISE BE A 500. A conversation can name an
/// agent that is not a managed, enabled, running one — an imported agent, a
/// retired one, a chat whose agent was never containerized — and the crons
/// crate says so in a sentence. The route has to hand that sentence to the
/// person at a 4xx, and leave no half-written row behind.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_agent_with_no_running_container_is_a_sentence_not_a_500() {
    let state = live_app_state().await;
    sweep(&state.pg, "idle").await;
    let (admin, cookie) = signed_in(&state, "idle", "admin").await;
    let (_, model) = agent_def(&state.pg, "idle", Some(&admin.id)).await;
    let conv = chat(&state.pg, &admin.id, &model).await;

    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/schedule"),
        Some(&cookie),
        Some(json!({ "name": "nightly", "schedule": "0 9 * * *", "prompt": "summarize" })),
    )
    .await;
    assert!(
        status.is_client_error(),
        "a sentence, not a 500 — got {status} {body}"
    );
    let said = body["error"].as_str().unwrap_or_default();
    assert!(
        said.contains("not a running managed agent"),
        "the crons crate's sentence reaches the caller, got {said:?}"
    );
    assert_eq!(schedule_count(&state.pg, &conv).await, 0);
}

/// What the header will read: the stored row, and a null live half wherever
/// the container cannot be asked — a schedule Talaria knows about must list
/// even when the agent is unreachable.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_stored_schedule_lists_for_the_thread() {
    let state = live_app_state().await;
    sweep(&state.pg, "list").await;
    let (owner, cookie) = signed_in(&state, "list", "member").await;
    let (_, stranger) = signed_in(&state, "list", "member").await;
    let (_, model) = agent_def(&state.pg, "list", Some(&owner.id)).await;
    let conv = chat(&state.pg, &owner.id, &model).await;
    let job = job_id();
    schedule_row(&state.pg, &model, &job, &conv, &owner.id).await;

    let path = format!("/api/conversations/{conv}/schedules");
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["schedules"].as_array().expect("a schedules list");
    assert_eq!(rows.len(), 1, "{body}");
    let row = &rows[0];
    assert_eq!(row["jobId"], job.as_str());
    assert_eq!(row["conversationId"], conv.as_str());
    assert_eq!(row["name"], json!("nightly digest"));
    assert_eq!(row["schedule"], json!("0 9 * * *"));
    let at = row["createdAt"].as_str().expect("an ISO created time");
    assert!(at.contains('T'), "ISO time, got {at}");
    // No container backs a fabricated agent, so the live half of the answer
    // is missing: every one of its fields comes back null and the stored row
    // lists anyway — which is the whole point of keeping the halves apart.
    for field in ["enabled", "state", "nextRunAt", "lastRunAt", "lastStatus"] {
        assert!(row[field].is_null(), "{field} with no container, got {row}");
    }

    // Reading the schedules is reading the thread: same door, same rules.
    let (status, _) = call_json(&state, "GET", &path, Some(&stranger), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(&state, "GET", &path, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── The agent's door: POST /api/agents/cron-output ──────────────────────────

/// THE DELIVERY. A cron run's final text arrives on the agent's own
/// credential and lands as an ordinary assistant turn in the thread that
/// asked for it — once, however many times the plugin's queue retries it,
/// because the marker carries the job, the execution and the turn.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_cron_turn_lands_in_the_thread_exactly_once() {
    let state = live_app_state().await;
    sweep(&state.pg, "out").await;
    // The notify rung reads the conversation's owner through a boot-injected
    // seam; a test binary never runs the arming path, so it is wired here.
    wire_boot_seams();
    let (owner, _) = signed_in(&state, "out", "member").await;
    let (def, model) = agent_def(&state.pg, "out", Some(&owner.id)).await;
    let key = agent_key(&state, &def).await;
    let conv = chat(&state.pg, &owner.id, &model).await;
    let job = job_id();
    schedule_row(&state.pg, &model, &job, &conv, &owner.id).await;

    let delivery = json!({
        "sessionId": format!("sess-{job}"),
        "taskId": format!("cron:{job}:abc123"),
        "turnId": "turn-1",
        "text": "the overnight digest",
    });
    let (status, body) = call_as_agent(&state, &key, delivery.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true));
    let message_id = body["messageId"]
        .as_str()
        .expect("the route answers with the id it wrote")
        .to_string();

    let (id, role, content, msg_status, marker) = last_turn(&state.pg, &conv).await;
    assert_eq!(id, message_id);
    assert_eq!(role, "assistant");
    assert_eq!(content, "the overnight digest");
    assert_eq!(msg_status, "complete");
    let want = format!("cron:{job}:abc123:turn-1");
    assert_eq!(marker.as_deref(), Some(want.as_str()));

    // The replay. The plugin's queue retries a delivery it could not confirm,
    // and a retried cron run must not say the same thing twice.
    let (status, again) = call_as_agent(&state, &key, delivery).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["messageId"], json!(message_id));
    let turns: i64 = sqlx::query_scalar(
        "select count(*) from messages where conversation_id = $1::uuid and role = 'assistant'",
    )
    .bind(&conv)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(turns, 1, "a replayed delivery writes nothing");

    // A turn that landed while nobody was looking rings once, exactly as a
    // streamed reply does — the whole reason the cron goes to the thread.
    assert!(
        await_agent_reply(&state.pg, &owner.id).await,
        "the cron turn files an agent-reply for the thread's owner"
    );
}

/// The platform does not trust the plugin's clamps: a credential-shaped span
/// in a cron's own output is redacted HERE, at the boundary, before it becomes
/// a message anyone can read.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_key_shaped_span_is_scrubbed_before_it_lands() {
    let state = live_app_state().await;
    sweep(&state.pg, "scrub").await;
    wire_boot_seams();
    let (owner, _) = signed_in(&state, "scrub", "member").await;
    let (def, model) = agent_def(&state.pg, "scrub", Some(&owner.id)).await;
    let key = agent_key(&state, &def).await;
    let conv = chat(&state.pg, &owner.id, &model).await;
    let job = job_id();
    schedule_row(&state.pg, &model, &job, &conv, &owner.id).await;

    let (status, body) = call_as_agent(
        &state,
        &key,
        json!({
            "sessionId": format!("sess-{job}"),
            "taskId": format!("cron:{job}:def456"),
            "turnId": "turn-1",
            "text": "the deploy key is AKIAIOSFODNN7EXAMPLE — rotate it",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, content, _, _) = last_turn(&state.pg, &conv).await;
    assert!(
        !content.contains("AKIAIOSFODNN7EXAMPLE"),
        "the raw key reached the transcript: {content}"
    );
    assert!(content.contains("[redacted:key]"), "got {content}");
}

/// A task id nobody is listening to, and a task id that is not a cron's at
/// all. Both are the agent's mistake, not the platform's, and they are
/// different mistakes: an unknown job is a 404, an unparseable task id a 400.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_unheard_or_unparseable_task_id_is_refused() {
    let state = live_app_state().await;
    sweep(&state.pg, "lost").await;
    let (owner, _) = signed_in(&state, "lost", "member").await;
    let (def, model) = agent_def(&state.pg, "lost", Some(&owner.id)).await;
    let key = agent_key(&state, &def).await;
    let conv = chat(&state.pg, &owner.id, &model).await;
    schedule_row(&state.pg, &model, &job_id(), &conv, &owner.id).await;

    // Well-formed, and this agent has a schedule — but not THIS job.
    let (status, body) = call_as_agent(
        &state,
        &key,
        json!({ "sessionId": "s", "taskId": "cron:deadbeef:1", "text": "orphan" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let said = body["error"].as_str().unwrap_or_default();
    assert!(said.contains("listening"), "got {said:?}");

    // Not a cron task id at all, and a job id that is not in the grammar —
    // "zzz" is not hex, so it never names a job and is refused at the parse,
    // one door earlier than the lookup above.
    for bad in ["notcron", "cron:zzz:1", "cron::abc123", "cron:deadbeef"] {
        let (status, body) = call_as_agent(
            &state,
            &key,
            json!({ "sessionId": "s", "taskId": bad, "text": "orphan" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} → {status} {body}");
    }

    // Nothing above wrote a turn.
    let turns: i64 = sqlx::query_scalar(
        "select count(*) from messages where conversation_id = $1::uuid and role = 'assistant'",
    )
    .bind(&conv)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(turns, 0);
}

/// A session cookie is not an agent, and the agent door says so — the chat's
/// own owner cannot post a cron turn into their thread by hand.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_agent_door_takes_no_session() {
    let state = live_app_state().await;
    sweep(&state.pg, "door").await;
    let (owner, cookie) = signed_in(&state, "door", "member").await;
    let (_, model) = agent_def(&state.pg, "door", Some(&owner.id)).await;
    let conv = chat(&state.pg, &owner.id, &model).await;
    let job = job_id();
    schedule_row(&state.pg, &model, &job, &conv, &owner.id).await;

    let (status, _) = call_json(
        &state,
        "POST",
        "/api/agents/cron-output",
        Some(&cookie),
        Some(json!({ "sessionId": "s", "taskId": format!("cron:{job}:abc123"), "text": "hi" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

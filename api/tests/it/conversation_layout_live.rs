// Live-DB + live-Redis proof for session organisation: the viewer's own pin,
// group and hand-placed position on a conversation row, and the agent door
// that files the session it is speaking in.
//
// WHY ONLY A REAL DATABASE PROVES IT. Every claim here is a WHERE clause or an
// ORDER BY: that the layout row is keyed by (viewer, conversation) and so a
// collaborator's pin never reaches the owner's rail; that the rail orders
// pinned first, then hand-placed slots, then newest activity; that a row whose
// three values are all default is DELETED rather than stored; and that the
// agent door resolves its conversation from the turn that is actually
// streaming. A unit test can only re-assert the SQL it is mirroring.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it conversation_layout_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::conversations::{
    create_conversation, insert_streaming_assistant, insert_user_message,
};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "conv-layout-live";

/// A throwaway person for ONE test. The binary runs its cases concurrently and
/// the rail assertions below read a whole list, so every test needs a viewer
/// whose list contains only its own rows — hence a uuid per person and the
/// test's own tag in the sub.
async fn member(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@{PREFIX}-{tag}.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

/// A chat whose activity time is hand-set. The ordering claims are about
/// oldest vs newest, and three inserts a microsecond apart would make them a
/// race rather than an assertion.
async fn chat(state: &AppState, owner: &str, title: &str, hours_ago: i32) -> String {
    let id = create_conversation(&state.pg, owner, "assistant-live", title, "chat", None)
        .await
        .unwrap();
    sqlx::query(
        "update conversations set updated_at = now() - make_interval(hours => $2::int) \
         where id = $1::uuid",
    )
    .bind(&id)
    .bind(hours_ago)
    .execute(&state.pg)
    .await
    .unwrap();
    id
}

async fn plan(state: &AppState, owner: &str, title: &str) -> String {
    create_conversation(&state.pg, owner, "assistant-live", title, "plan", None)
        .await
        .unwrap()
}

async fn share(pg: &PgPool, conversation: &str, user: &str) {
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
    )
    .bind(conversation)
    .bind(user)
    .execute(pg)
    .await
    .unwrap();
}

/// A fleet agent with its own credential: an agent_defs row (the model names no
/// real provider, so nothing dispatches it) and a minted per-agent key — the
/// exact credential a container presents. slug and model are each UNIQUE on
/// agent_defs, so both carry a uuid and a failed run's leftovers cannot collide
/// with the next one. Returns (model, key).
async fn fleet_agent(state: &AppState, tag: &str) -> (String, String) {
    let model = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'engineering', $2, 'Conversation Layout Live Agent') returning id::text",
    )
    .bind(&model)
    .bind(&model)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, &id)
        .await
        .expect("agent key mints");
    (model, key)
}

/// One request through the REAL router with an AGENT's credential. `call_json`
/// carries a session cookie and nothing else, and this door is agent-key only.
async fn call_agent(
    state: &AppState,
    method: &str,
    path: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("x-api-key", key)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = talaria_api::routes::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// The viewer's rail, as the client reads it.
async fn rail(state: &AppState, cookie: &str, kind: &str) -> Vec<Value> {
    let (status, body) = call_json(
        state,
        "GET",
        &format!("/api/conversations?kind={kind}"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["conversations"]
        .as_array()
        .expect("the list answers an array")
        .clone()
}

fn ids(rows: &[Value]) -> Vec<&str> {
    rows.iter()
        .map(|r| r["id"].as_str().expect("every row carries an id"))
        .collect()
}

fn rail_row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["id"].as_str() == Some(id))
        .expect("the conversation is in this viewer's rail")
}

async fn patch_ok(state: &AppState, cookie: &str, id: &str, body: Value) {
    let (status, answer) = call_json(
        state,
        "PATCH",
        &format!("/api/conversations/{id}"),
        Some(cookie),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body} → {answer}");
}

async fn layout_rows(pg: &PgPool, user_id: &str, conversation_id: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from conversation_layout \
         where user_id = $1::uuid and conversation_id = $2::uuid",
    )
    .bind(user_id)
    .bind(conversation_id)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// The rail's order, and the fields it carries: pinned first, then a
/// hand-placed slot, then newest activity — the last of which is the ordering
/// the rail has always had and which organising must not take away.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_pin_and_a_position_reorder_the_rail() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "order").await;
    let oldest = chat(&state, &me, "Oldest", 3).await;
    let middle = chat(&state, &me, "Middle", 2).await;
    let newest = chat(&state, &me, "Newest", 1).await;

    let rows = rail(&state, &cookie, "chat").await;
    assert_eq!(
        ids(&rows),
        vec![newest.as_str(), middle.as_str(), oldest.as_str()],
        "newest activity first, before anyone organises anything"
    );
    // The fields are on every row, organised or not: a client that has to
    // guess whether `pinned` is false or missing has two code paths for one
    // state.
    for row in &rows {
        assert_eq!(row["pinned"], json!(false), "{row}");
        assert_eq!(row["group"], Value::Null, "{row}");
        assert_eq!(row["position"], Value::Null, "{row}");
    }

    patch_ok(&state, &cookie, &oldest, json!({ "pinned": true })).await;
    let rows = rail(&state, &cookie, "chat").await;
    assert_eq!(
        ids(&rows)[0],
        oldest.as_str(),
        "a pinned thread leads the rail however old it is"
    );
    assert_eq!(rail_row(&rows, &oldest)["pinned"], json!(true));

    // A second PATCH naming only `group` MERGES into the same row: the pin it
    // does not mention survives. (And the group is stored trimmed.)
    patch_ok(&state, &cookie, &oldest, json!({ "group": "  Keep  " })).await;
    let rows = rail(&state, &cookie, "chat").await;
    let row = rail_row(&rows, &oldest);
    assert_eq!(row["group"], json!("Keep"), "{row}");
    assert_eq!(
        row["pinned"],
        json!(true),
        "a field the patch did not name is not a field it cleared"
    );

    // A hand-placed slot jumps the middle thread past the newer one, and still
    // sits behind the pin.
    patch_ok(&state, &cookie, &middle, json!({ "position": 0 })).await;
    let rows = rail(&state, &cookie, "chat").await;
    assert_eq!(
        ids(&rows),
        vec![oldest.as_str(), middle.as_str(), newest.as_str()]
    );
    let row = rail_row(&rows, &middle);
    assert_eq!(row["position"], json!(0), "{row}");
    assert_eq!(row["pinned"], json!(false), "{row}");
}

/// THE POINT OF THE TABLE: pin, group and position belong to the person
/// looking, not to the thread. A shared plan is the case that proves it —
/// a collaborator can organise their own rail and the owner never sees it.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_collaborators_pin_never_reaches_the_owners_rail() {
    let state = live_app_state().await;
    let (owner, owner_cookie) = member(&state, "viewer-owner").await;
    let (mate, mate_cookie) = member(&state, "viewer-mate").await;
    let shared = plan(&state, &owner, "Shared plan").await;
    share(&state.pg, &shared, &mate).await;

    patch_ok(
        &state,
        &mate_cookie,
        &shared,
        json!({ "pinned": true, "group": "Mine" }),
    )
    .await;

    let theirs = rail(&state, &mate_cookie, "plan").await;
    let row = rail_row(&theirs, &shared);
    assert_eq!(row["pinned"], json!(true), "{row}");
    assert_eq!(row["group"], json!("Mine"), "{row}");
    assert_eq!(row["role"], json!("collaborator"), "{row}");

    let ours = rail(&state, &owner_cookie, "plan").await;
    let row = rail_row(&ours, &shared);
    assert_eq!(
        row["pinned"],
        json!(false),
        "the owner's rail is the owner's own: {row}"
    );
    assert_eq!(row["group"], Value::Null, "{row}");
    assert_eq!(row["position"], Value::Null, "{row}");
}

/// Organising nothing leaves nothing behind. The row exists to hold a
/// decision; once every value is back at its default there is no decision, and
/// a table full of all-default rows is a table that has to be swept later.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn clearing_every_field_deletes_the_row() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "default").await;
    let conv = chat(&state, &me, "Tidy", 1).await;

    patch_ok(
        &state,
        &cookie,
        &conv,
        json!({ "pinned": true, "group": "Keep", "position": 3 }),
    )
    .await;
    assert_eq!(layout_rows(&state.pg, &me, &conv).await, 1);

    patch_ok(
        &state,
        &cookie,
        &conv,
        json!({ "pinned": false, "group": null, "position": null }),
    )
    .await;
    assert_eq!(
        layout_rows(&state.pg, &me, &conv).await,
        0,
        "a row that says nothing is deleted, not stored"
    );
    // And the rail still answers for it — the absent row reads as the default.
    let rows = rail(&state, &cookie, "chat").await;
    let row = rail_row(&rows, &conv);
    assert_eq!(row["pinned"], json!(false), "{row}");
    assert_eq!(row["group"], Value::Null, "{row}");
    assert_eq!(row["position"], Value::Null, "{row}");
}

/// The bounds, and the access gate. Each refusal is a 400 the model (or the
/// client) can read; a stranger gets the PATCH route's existing answer, which
/// is the read's 404 — the gate IS the read, so probing an id learns nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_impossible_organisation_is_refused() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "bounds").await;
    let (_, stranger_cookie) = member(&state, "bounds-stranger").await;
    let conv = chat(&state, &me, "Validate", 1).await;
    let path = format!("/api/conversations/{conv}");

    for bad in [
        json!({ "group": "x".repeat(41) }),
        json!({ "group": "   " }),
        json!({ "position": -1 }),
        json!({ "position": 100_001 }),
        // Nothing to change: a write that names no field is a mistake, not a
        // no-op to shrug at.
        json!({}),
    ] {
        let (status, body) =
            call_json(&state, "PATCH", &path, Some(&cookie), Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} → {body}");
    }

    let (status, body) = call_json(
        &state,
        "PATCH",
        &path,
        Some(&stranger_cookie),
        Some(json!({ "pinned": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// The agent door. No conversation parameter, deliberately — the same reason
/// PUT /api/agent/todo has none: the agent is speaking in a thread right now,
/// Talaria knows which one, and a model that had to pass the id could pass a
/// stale one or somebody else's. The streaming assistant row below is that
/// turn, and it is what `speaking_target` resolves the conversation from.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_agent_mid_turn_files_the_session_it_is_in() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "agent").await;
    let (model, key) = fleet_agent(&state, "organise").await;
    let conv = create_conversation(&state.pg, &owner, &model, "Untitled", "chat", None)
        .await
        .unwrap();
    insert_user_message(
        &state.pg,
        &conv,
        0,
        "file this away when you're done",
        &json!([]),
        Some(&owner),
        &json!({}),
    )
    .await
    .unwrap();
    insert_streaming_assistant(&state.pg, &conv, 1, &json!({}))
        .await
        .unwrap();

    let (status, body) = call_agent(
        &state,
        "POST",
        "/api/agent/organize",
        &key,
        json!({ "title": "Filed", "group": "Done" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "{body}");
    // `changed` is the set of fields that landed — what the model relays back.
    // Sorted, because WHICH fields applied is the contract and the order they
    // are listed in is not.
    let mut changed: Vec<&str> = body["changed"]
        .as_array()
        .expect("changed names the fields applied")
        .iter()
        .map(|v| v.as_str().expect("field names are strings"))
        .collect();
    changed.sort_unstable();
    assert_eq!(changed, vec!["group", "title"], "{body}");
    assert!(
        body["said"].as_str().is_some_and(|s| !s.trim().is_empty()),
        "one sentence the model can say out loud: {body}"
    );

    let rows = rail(&state, &cookie, "chat").await;
    let row = rail_row(&rows, &conv);
    assert_eq!(row["title"], json!("Filed"), "{row}");
    assert_eq!(
        row["group"],
        json!("Done"),
        "the OWNER's layout row — the owner is who reads this rail: {row}"
    );

    // An agent with no live turn has no conversation to file, and is told so
    // rather than writing somewhere plausible.
    let (_, idle_key) = fleet_agent(&state, "idle").await;
    let (status, body) = call_agent(
        &state,
        "POST",
        "/api/agent/organize",
        &idle_key,
        json!({ "title": "Filed" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

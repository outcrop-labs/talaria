// Live-DB + live-Redis proof of POST /api/agent/open — an agent putting an
// existing file in the pane beside the session it is answering.
//
// WHY ONLY A REAL DATABASE PROVES IT. Every claim is a join or an ORDER BY:
// that the chip lands on the streaming row of THIS agent's conversation; that
// the person a turn is for is the author of the newest person's message BELOW
// that row (a collaborator's message queued behind it never counts); that the
// files route then lists the file as touched, for that person, at that row's
// time; and that a document the person cannot read lands nothing at all.
//
//   source ui/.env && cargo test -p talaria-api --test it agent_open_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::conversations::{
    create_conversation, insert_streaming_assistant, insert_user_message,
};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "agent-open-live";

/// A throwaway person for ONE test: a uuid per person, the test's tag in the
/// sub, so concurrent cases never read each other's rows.
async fn member(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@{PREFIX}-{tag}.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

/// A fleet agent with its own credential — the exact key a container presents.
async fn fleet_agent(state: &AppState, tag: &str) -> (String, String) {
    let model = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'engineering', $2, 'Agent Open Live Agent') returning id::text",
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

/// One request through the REAL router with an agent's credential.
async fn open(state: &AppState, key: &str, body: Value) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/agent/open")
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

/// An artifact row, as the Files page would have made it.
async fn artifact(
    pg: &PgPool,
    owner: &str,
    title: &str,
    kind: &str,
    visibility: &str,
    edit_policy: &str,
) -> String {
    sqlx::query_scalar(
        "insert into artifacts (kind, title, body, visibility, edit_policy, owner_user_id) \
         values ($1, $2, 'body text', $3, $4, $5::uuid) returning id::text",
    )
    .bind(kind)
    .bind(title)
    .bind(visibility)
    .bind(edit_policy)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn grant(pg: &PgPool, artifact: &str, principal_type: &str, principal: &str, role: &str) {
    sqlx::query(
        "insert into kb_editors (item_type, item_id, principal_type, principal_id, role) \
         values ('artifact', $1::uuid, $2, $3, $4)",
    )
    .bind(artifact)
    .bind(principal_type)
    .bind(principal)
    .bind(role)
    .execute(pg)
    .await
    .unwrap();
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

/// A session of `kind` with the person's message at seq 0 and the agent's
/// reply streaming at seq 1 — the turn the door lands on. Returns
/// (conversation, live row).
async fn live_turn(state: &AppState, owner: &str, model: &str, kind: &str) -> (String, String) {
    let conv = create_conversation(&state.pg, owner, model, "Session", kind, None)
        .await
        .unwrap();
    insert_user_message(
        &state.pg,
        &conv,
        0,
        "open the plan here so we can iterate on it",
        &json!([]),
        Some(owner),
        &json!({}),
    )
    .await
    .unwrap();
    let row = insert_streaming_assistant(&state.pg, &conv, 1, &json!({}))
        .await
        .unwrap();
    (conv, row)
}

async fn chips_of(pg: &PgPool, message: &str) -> Value {
    sqlx::query_scalar("select chips from messages where id = $1::uuid")
        .bind(message)
        .fetch_one(pg)
        .await
        .unwrap()
}

fn names(chips: &Value, href: &str) -> bool {
    chips
        .as_array()
        .is_some_and(|all| all.iter().any(|c| c["href"].as_str() == Some(href)))
}

/// The session's files as one person's pane reads them.
async fn files(state: &AppState, cookie: &str, conv: &str) -> Vec<Value> {
    let (status, body) = call_json(
        state,
        "GET",
        &format!("/api/conversations/{conv}/files"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["files"].as_array().expect("files is an array").clone()
}

fn file<'a>(files: &'a [Value], key: &str) -> Option<&'a Value> {
    files.iter().find(|f| f["key"].as_str() == Some(key))
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_agent_mid_turn_opens_an_existing_document_in_the_persons_pane() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "open-owner").await;
    let (author, _) = member(&state, "open-author").await;
    let (model, key) = fleet_agent(&state, "open").await;
    let (conv, row) = live_turn(&state, &owner, &model, "work").await;
    let doc = artifact(&state.pg, &author, "Launch plan", "doc", "org", "org").await;

    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["conversationId"], json!(conv), "{body}");
    assert_eq!(body["file"]["title"], json!("Launch plan"), "{body}");
    assert_eq!(body["file"]["type"], json!("document"), "{body}");
    assert_eq!(
        body["theyCanEdit"],
        json!(true),
        "an org edit policy: {body}"
    );
    assert_eq!(
        body["youCanEdit"],
        json!(false),
        "no grant, no edit: {body}"
    );
    assert_eq!(body["alreadyOpen"], json!(false), "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("Opened “Launch plan” in their pane."),
        "{message}"
    );
    assert!(message.contains("make you an editor"), "{message}");

    // THE SAME CHIP A CREATE LANDS, on the streaming row.
    let href = format!("/artifacts?a={doc}");
    assert!(names(&chips_of(&state.pg, &row).await, &href));

    // And the pane lists it — touched, for the person who asked.
    let listed = files(&state, &cookie, &conv).await;
    let f = file(&listed, &format!("artifact:{doc}")).expect("the pane lists it");
    assert_eq!(f["source"], json!("touched"), "{f}");
    assert_eq!(f["touchedFor"], json!(owner), "{f}");
    assert!(f["lastTouchedAt"].is_string(), "{f}");

    // A second open on the same turn moves nothing, and says so.
    let (status, again) = open(&state, &key, json!({ "url": href })).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["alreadyOpen"], json!(true), "{again}");
    let message = again["message"].as_str().unwrap_or_default();
    assert!(message.contains("already linked on this turn"), "{message}");
    assert!(!message.contains("Opened"), "{message}");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn reopening_a_file_from_an_earlier_turn_moves_its_touch_to_the_live_turn() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "reopen").await;
    let (model, key) = fleet_agent(&state, "reopen").await;
    let doc = artifact(&state.pg, &owner, "Q3 numbers", "sheet", "org", "owner").await;
    let conv = create_conversation(&state.pg, &owner, &model, "Session", "work", None)
        .await
        .unwrap();
    insert_user_message(
        &state.pg,
        &conv,
        0,
        "make the sheet",
        &json!([]),
        Some(&owner),
        &json!({}),
    )
    .await
    .unwrap();
    // The turn an hour ago that made it.
    sqlx::query(
        "insert into messages (conversation_id, seq, role, content, status, chips, created_at) \
         values ($1::uuid, 1, 'assistant', 'made it', 'complete', $2, now() - interval '1 hour')",
    )
    .bind(&conv)
    .bind(
        json!([{ "id": format!("link:/artifacts?a={doc}"), "kind": "link",
                   "href": format!("/artifacts?a={doc}"), "entity": "document" }]),
    )
    .execute(&state.pg)
    .await
    .unwrap();
    insert_user_message(
        &state.pg,
        &conv,
        2,
        "bring the sheet back",
        &json!([]),
        Some(&owner),
        &json!({}),
    )
    .await
    .unwrap();
    insert_streaming_assistant(&state.pg, &conv, 3, &json!({}))
        .await
        .unwrap();

    let key_of = format!("artifact:{doc}");
    let before = files(&state, &cookie, &conv).await;
    let was = file(&before, &key_of).expect("listed from the earlier turn")["lastTouchedAt"]
        .as_str()
        .expect("touched")
        .to_string();

    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["alreadyOpen"],
        json!(false),
        "a different turn: {body}"
    );

    let after = files(&state, &cookie, &conv).await;
    let now = file(&after, &key_of).expect("still listed")["lastTouchedAt"]
        .as_str()
        .expect("touched")
        .to_string();
    assert!(now > was, "the touch moved to the live turn: {was} → {now}");
    assert_eq!(after.len(), 1, "one file, not a second copy: {after:?}");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_document_the_person_cannot_read_lands_nothing() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "unreadable").await;
    let (stranger, _) = member(&state, "unreadable-other").await;
    let (model, key) = fleet_agent(&state, "unreadable").await;
    let (conv, row) = live_turn(&state, &owner, &model, "work").await;
    // Private to someone else, shared with the agent only.
    let doc = artifact(&state.pg, &stranger, "Salaries", "doc", "private", "owner").await;
    grant(&state.pg, &doc, "agent", &model, "viewer").await;

    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("cannot read that document")),
        "{body}"
    );
    assert_eq!(chips_of(&state.pg, &row).await, json!([]), "nothing landed");
    assert!(files(&state, &cookie, &conv).await.is_empty());
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_person_without_edit_rights_gets_it_view_only() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "viewonly").await;
    let (author, _) = member(&state, "viewonly-author").await;
    let (model, key) = fleet_agent(&state, "viewonly").await;
    let _ = live_turn(&state, &owner, &model, "work").await;
    let doc = artifact(&state.pg, &author, "Pricing notes", "doc", "org", "owner").await;

    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["theyCanEdit"], json!(false), "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("opens view-only"), "{message}");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_document_the_agent_cannot_read_opens_and_tells_the_agent_nothing() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "agent-blind").await;
    let (model, key) = fleet_agent(&state, "agent-blind").await;
    let (conv, row) = live_turn(&state, &owner, &model, "work").await;
    // The person's own private document, never shared with the agent.
    let doc = artifact(
        &state.pg,
        &owner,
        "My private draft",
        "doc",
        "private",
        "owner",
    )
    .await;

    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["file"].get("title").is_none(), "{body}");
    assert_eq!(body["youCanEdit"], Value::Null, "{body}");
    let text = body.to_string();
    assert!(!text.contains("My private draft"), "{text}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("You cannot read it yourself")),
        "{body}"
    );
    // The chip carries no title either; the person's pane names it from the row.
    let chips = chips_of(&state.pg, &row).await;
    assert!(!chips.to_string().contains("My private draft"), "{chips}");
    let listed = files(&state, &cookie, &conv).await;
    let f = file(&listed, &format!("artifact:{doc}")).expect("listed for its owner");
    assert_eq!(f["title"], json!("My private draft"), "{f}");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_google_link_opens_as_a_google_file() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "google").await;
    let (model, key) = fleet_agent(&state, "google").await;
    let (conv, _) = live_turn(&state, &owner, &model, "work").await;

    let (status, body) = open(
        &state,
        &key,
        json!({ "url": "https://docs.google.com/spreadsheets/d/1liveSheetForOpen/edit#gid=0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["file"]["kind"], json!("google"), "{body}");
    assert_eq!(body["theyCanEdit"], Value::Null, "Google decides: {body}");
    let listed = files(&state, &cookie, &conv).await;
    let f = file(&listed, "google:1liveSheetForOpen").expect("the pane lists it");
    assert_eq!(f["googleType"], json!("spreadsheet"), "{f}");
    assert_eq!(f["touchedFor"], json!(owner), "{f}");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn what_the_pane_cannot_show_is_refused_and_lands_nothing() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "refused").await;
    let (model, key) = fleet_agent(&state, "refused").await;
    let (_, row) = live_turn(&state, &owner, &model, "work").await;
    let doc = artifact(&state.pg, &owner, "Plan", "doc", "org", "owner").await;
    let transcript = artifact(
        &state.pg,
        &owner,
        "Run log",
        "run-transcript",
        "org",
        "owner",
    )
    .await;

    for (body, want, says) in [
        (
            json!({ "url": format!("/knowledge?doc={}", uuid::Uuid::new_v4()) }),
            StatusCode::BAD_REQUEST,
            "Knowledge page",
        ),
        (
            json!({ "documentId": doc, "url": "https://docs.google.com/document/d/1x/edit" }),
            StatusCode::BAD_REQUEST,
            "one of them",
        ),
        (
            json!({ "documentId": "1GoogleIdNotAUuid" }),
            StatusCode::BAD_REQUEST,
            "Talaria document id",
        ),
        (
            json!({ "documentId": transcript }),
            StatusCode::BAD_REQUEST,
            "run view",
        ),
        (
            json!({ "documentId": uuid::Uuid::new_v4().to_string() }),
            StatusCode::NOT_FOUND,
            "not found",
        ),
    ] {
        let (status, answer) = open(&state, &key, body.clone()).await;
        assert_eq!(status, want, "{body} → {answer}");
        assert!(
            answer["error"].as_str().is_some_and(|e| e.contains(says)),
            "{body} → {answer}"
        );
    }
    assert_eq!(chips_of(&state.pg, &row).await, json!([]), "nothing landed");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn no_turn_several_turns_and_a_foreign_conversation_are_refused() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "where").await;
    let doc = artifact(&state.pg, &owner, "Plan", "doc", "org", "owner").await;

    // Not speaking anywhere: nobody to open it for.
    let (_, idle) = fleet_agent(&state, "where-idle").await;
    let (status, body) = open(&state, &idle, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Two live turns and no conversation named: never a guess.
    let (model, key) = fleet_agent(&state, "where-busy").await;
    let (first, first_row) = live_turn(&state, &owner, &model, "work").await;
    let (_, second_row) = live_turn(&state, &owner, &model, "work").await;
    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("conversationId")),
        "{body}"
    );
    // Named, it lands there and only there.
    let (status, body) = open(
        &state,
        &key,
        json!({ "documentId": doc, "conversationId": first }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let href = format!("/artifacts?a={doc}");
    assert!(names(&chips_of(&state.pg, &first_row).await, &href));
    assert!(!names(&chips_of(&state.pg, &second_row).await, &href));

    // Another agent's conversation is not one this key can reach.
    let (other_model, _) = fleet_agent(&state, "where-other").await;
    let (foreign, foreign_row) = live_turn(&state, &owner, &other_model, "work").await;
    let (status, body) = open(
        &state,
        &key,
        json!({ "documentId": doc, "conversationId": foreign }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(chips_of(&state.pg, &foreign_row).await, json!([]));
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_turn_is_for_whoever_sent_the_message_it_answers_never_a_queued_one() {
    let state = live_app_state().await;
    let (owner, owner_cookie) = member(&state, "who-owner").await;
    let (mate, mate_cookie) = member(&state, "who-mate").await;
    let (model, key) = fleet_agent(&state, "who").await;
    let conv = create_conversation(&state.pg, &owner, &model, "Shared", "work", None)
        .await
        .unwrap();
    share(&state.pg, &conv, &mate).await;
    // The collaborator asked; the agent is answering them; the owner's message
    // is queued BEHIND the live reply.
    insert_user_message(
        &state.pg,
        &conv,
        0,
        "open my notes",
        &json!([]),
        Some(&mate),
        &json!({}),
    )
    .await
    .unwrap();
    insert_streaming_assistant(&state.pg, &conv, 1, &json!({}))
        .await
        .unwrap();
    insert_user_message(
        &state.pg,
        &conv,
        2,
        "and then mine",
        &json!([]),
        Some(&owner),
        &json!({}),
    )
    .await
    .unwrap();

    // The owner's private doc: the collaborator cannot read it, so it is
    // refused — the queued owner message does not make the turn the owner's.
    let owners = artifact(&state.pg, &owner, "Owner only", "doc", "private", "owner").await;
    grant(&state.pg, &owners, "agent", &model, "viewer").await;
    let (status, body) = open(&state, &key, json!({ "documentId": owners })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // The collaborator's own private doc opens, for them.
    let mates = artifact(&state.pg, &mate, "Mate notes", "doc", "private", "owner").await;
    let (status, body) = open(&state, &key, json!({ "documentId": mates })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["theyCanEdit"], json!(true), "their own doc: {body}");
    let theirs = files(&state, &mate_cookie, &conv).await;
    let f = file(&theirs, &format!("artifact:{mates}")).expect("in the collaborator's pane");
    assert_eq!(f["touchedFor"], json!(mate), "{f}");
    // The owner cannot read it, so their pane never lists it.
    let ours = files(&state, &owner_cookie, &conv).await;
    assert!(
        file(&ours, &format!("artifact:{mates}")).is_none(),
        "{ours:?}"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn with_no_file_named_it_lists_the_session_files_the_agent_can_read() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "list").await;
    let (model, key) = fleet_agent(&state, "list").await;
    let (conv, _) = live_turn(&state, &owner, &model, "work").await;
    let shared = artifact(&state.pg, &owner, "Shared plan", "doc", "org", "owner").await;
    let private = artifact(
        &state.pg,
        &owner,
        "Private draft",
        "doc",
        "private",
        "owner",
    )
    .await;
    sqlx::query("update conversations set pinned_files = $2 where id = $1::uuid")
        .bind(&conv)
        .bind(json!([
            { "kind": "artifact", "id": shared },
            { "kind": "artifact", "id": private },
            { "kind": "google", "id": "1pinnedDocForList",
              "mime": "application/vnd.google-apps.document" },
        ]))
        .execute(&state.pg)
        .await
        .unwrap();

    let (status, body) = open(&state, &key, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = body["files"].as_array().expect("files").clone();
    assert!(
        listed.iter().any(|f| f["documentId"] == json!(shared)),
        "{body}"
    );
    assert!(
        listed.iter().any(|f| f["url"]
            .as_str()
            .is_some_and(|u| u.contains("1pinnedDocForList"))),
        "{body}"
    );
    // The person's private draft is in their pane, but not the agent's to see.
    assert!(!body.to_string().contains("Private draft"), "{body}");
    assert!(!body.to_string().contains(&private), "{body}");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_chat_without_a_pane_gets_the_link_and_says_so() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "chat").await;
    let (model, key) = fleet_agent(&state, "chat").await;
    let (_, row) = live_turn(&state, &owner, &model, "chat").await;
    let doc = artifact(&state.pg, &owner, "Plan", "doc", "org", "owner").await;

    let (status, body) = open(&state, &key, json!({ "documentId": doc })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("Linked “Plan” in this chat")),
        "{body}"
    );
    assert!(names(
        &chips_of(&state.pg, &row).await,
        &format!("/artifacts?a={doc}")
    ));
}

/// A DOCUMENT THE AGENT CREATES JOINS THE SESSION. The tool-events door reads
/// the create's answer, which the agent's plugin cuts to 4000 chars — a body
/// past that lost the id, and the file never reached the pane. The create
/// route lands the chip itself now, as the edit route does.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_document_the_agent_creates_joins_the_session_whatever_its_length() {
    use tower::ServiceExt;
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "create").await;
    let (model, key) = fleet_agent(&state, "create").await;
    let (conv, row) = live_turn(&state, &owner, &model, "work").await;

    let body = json!({ "kind": "doc", "title": "Long brief", "body": "word ".repeat(2_000) });
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/artifacts")
        .header("x-api-key", &key)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = talaria_api::routes::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: Value = serde_json::from_slice(&raw).unwrap();
    let id = created["artifact"]["id"]
        .as_str()
        .expect("an id")
        .to_string();

    // The chip lands detached, a beat after the answer.
    let href = format!("/artifacts?a={id}");
    let mut landed = false;
    for _ in 0..40 {
        if names(&chips_of(&state.pg, &row).await, &href) {
            landed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(landed, "the create's chip is on the live turn");
    let listed = files(&state, &cookie, &conv).await;
    let f = file(&listed, &format!("artifact:{id}")).expect("the pane lists it");
    assert_eq!(f["touchedFor"], json!(owner), "{f}");
}

/// One POST /api/artifacts through the REAL router with an agent's key;
/// answers the new document's id.
async fn agent_creates(state: &AppState, key: &str, title: &str) -> String {
    use tower::ServiceExt;
    let body = json!({ "kind": "doc", "title": title, "body": "a short body" });
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/artifacts")
        .header("x-api-key", key)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = talaria_api::routes::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: Value = serde_json::from_slice(&raw).unwrap();
    created["artifact"]["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// WITH NO LIVE TURN, NEITHER A CREATE NOR AN OPEN LANDS. A cron run or a
/// background job works while no reply of the agent's is streaming. The
/// unaddressed resolver's recent tier would land its file on whichever thread
/// the agent last spoke in — and a document chip opens the file in that
/// person's pane. The create route and the open door land only on a live
/// turn; the door answers 409, named conversation or not.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn with_no_live_turn_neither_a_create_nor_an_open_lands_on_a_recent_thread() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "detached").await;
    let (model, key) = fleet_agent(&state, "detached").await;
    // The agent answered this person a moment ago; that reply is finished.
    let (conv, row) = live_turn(&state, &owner, &model, "work").await;
    sqlx::query("update messages set status = 'complete' where id = $1::uuid")
        .bind(&row)
        .execute(&state.pg)
        .await
        .unwrap();
    // The recent tier sees that row: an unaddressed landing would pick it.
    let target = talaria_chips::landing_target(&state.pg, &model, None)
        .await
        .unwrap();
    assert_eq!(
        target.landed().map(|l| l.message_id),
        Some(row.clone()),
        "the setup: one recent thread"
    );

    let id = agent_creates(&state, &key, "Nightly digest").await;
    // The chip would land detached, a beat after the answer: wait it out.
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    let href = format!("/artifacts?a={id}");
    assert!(
        !names(&chips_of(&state.pg, &row).await, &href),
        "no chip on the finished reply"
    );
    let listed = files(&state, &cookie, &conv).await;
    assert!(
        file(&listed, &format!("artifact:{id}")).is_none(),
        "{listed:?}"
    );

    // The open door: nobody is waiting on a finished turn.
    let doc = artifact(&state.pg, &owner, "Daily report", "doc", "org", "owner").await;
    for body in [
        json!({ "documentId": doc }),
        json!({ "documentId": doc, "conversationId": conv }),
    ] {
        let (status, answer) = open(&state, &key, body.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}: {answer}");
    }
    assert!(
        !names(
            &chips_of(&state.pg, &row).await,
            &format!("/artifacts?a={doc}")
        ),
        "no open on the finished reply"
    );
}

/// An assistant row, finished, at `seq` — optionally carrying the
/// responsible person a cron turn stamps on its own row.
async fn finished_reply(pg: &PgPool, conv: &str, seq: i32, stamped: Option<&str>) -> String {
    let row = insert_streaming_assistant(pg, conv, seq, &json!({}))
        .await
        .unwrap();
    sqlx::query(
        "update messages set status = 'complete', author_user_id = $2::uuid where id = $1::uuid",
    )
    .bind(&row)
    .bind(stamped)
    .execute(pg)
    .await
    .unwrap();
    row
}

/// WHO A ROW WAS FOR, in the two cases the first reading got wrong.
/// Unattributed means the owner on both sides: a reply to a person's message
/// with no author is the owner's, never an older collaborator's further
/// back. And a cron turn's own row names the person who set it up.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn unattributed_means_the_owner_and_a_cron_turn_is_its_creators() {
    use talaria_api::conversations::turn_requester;
    let state = live_app_state().await;
    let (owner, _) = member(&state, "attr-owner").await;
    let (mate, _) = member(&state, "attr-mate").await;
    let (model, _) = fleet_agent(&state, "attr").await;
    let conv = create_conversation(&state.pg, &owner, &model, "Shared", "work", None)
        .await
        .unwrap();
    share(&state.pg, &conv, &mate).await;
    let pg = &state.pg;

    insert_user_message(pg, &conv, 0, "hi", &json!([]), Some(&owner), &json!({}))
        .await
        .unwrap();
    // A cron turn the collaborator scheduled, landing after the owner spoke.
    let cron = finished_reply(pg, &conv, 1, Some(&mate)).await;
    assert_eq!(turn_requester(pg, &cron).await.unwrap(), Some(mate.clone()));

    insert_user_message(
        pg,
        &conv,
        2,
        "open my notes",
        &json!([]),
        Some(&mate),
        &json!({}),
    )
    .await
    .unwrap();
    let theirs = finished_reply(pg, &conv, 3, None).await;
    assert_eq!(
        turn_requester(pg, &theirs).await.unwrap(),
        Some(mate.clone())
    );

    // A person's message with no author: the owner's.
    let unattributed = insert_user_message(pg, &conv, 4, "and this", &json!([]), None, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        turn_requester(pg, &unattributed).await.unwrap(),
        Some(owner.clone())
    );
    let reply = insert_streaming_assistant(pg, &conv, 5, &json!({}))
        .await
        .unwrap();
    assert_eq!(
        turn_requester(pg, &reply).await.unwrap(),
        Some(owner.clone()),
        "a reply to the unattributed message is the owner's, not the collaborator's above it"
    );
}

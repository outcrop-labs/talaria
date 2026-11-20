// task_review_exit_live (see main.rs): an agent takes a ticket out of review
// ONLY on a person's proven ask (talaria-tasks `asked`), against a real
// database through the REAL router — people with minted sessions, an agent
// with its own `tak_` key, and the agent's live turn staged as the rows the
// chat and channel doors write (a streaming reply, the person's message).
//
//   no ask       403 with the way through, the ticket unmoved
//   proven       200: the move, "(asked by <email> in chat)" by the agent, a
//                task.review_exit audit row naming the message, the sign-off
//                cleared; the same message a second time is Spent
//   not proven   another agent's thread, no live reply, a viewer's ask, a
//                message that does not name the ticket, words the person only
//                clicked, a cron's stamped reply with no person — all 403
//   terminal     done / failed / cancelled refused even when asked
//   the room     in the ticket's own discussion the ask need not name it
//   board move   the asker must edit BOTH boards; askedBy rides the audit row
//   the card     a refused move's card: a board editor (not the agent's owner,
//                not an admin) approves it and the move is theirs; a viewer
//                gets 403; a card carrying a ref resolves it
//
// House rule: #[ignore]d, run against a migrated database:
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… TALARIA_SECRET_KEY=… \
//     cargo test -p talaria-api --test it task_review_exit_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::routes;
use talaria_state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "task-review-exit-live";

async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from chat_approvals where agent_model like $1")
        .bind(format!("trx{tag}%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}-{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("trx{tag}%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn member(state: &AppState, tag: &str, who: &str) -> (String, String, String) {
    let email = format!(
        "{PREFIX}-{tag}-{who}-{}@exit-test.invalid",
        uuid::Uuid::new_v4().simple()
    );
    let (id, cookie) = minted_member_with_email(state, &format!("{PREFIX}-{tag}"), &email).await;
    (id, cookie, email)
}

async fn board(pg: &PgPool, owner: &str, name: &str, prefix: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id, ticket_prefix) values ($1, $2::uuid, $3) returning id::text",
    )
    .bind(name)
    .bind(owner)
    .bind(prefix)
    .fetch_one(pg)
    .await
    .unwrap();
    share(pg, &id, owner, "owner").await;
    id
}

async fn share(pg: &PgPool, board: &str, user: &str, role: &str) {
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, $3) \
         on conflict (board_id, user_id) do update set role = excluded.role",
    )
    .bind(board)
    .bind(user)
    .bind(role)
    .execute(pg)
    .await
    .unwrap();
}

/// A ticket in `status`, numbered off the board's counter; returns (id, ref).
async fn ticket(pg: &PgPool, board: &str, status: &str) -> (String, String) {
    let (no,): (i32,) = sqlx::query_as(
        "update boards set ticket_seq = ticket_seq + 1 where id = $1::uuid returning ticket_seq",
    )
    .bind(board)
    .fetch_one(pg)
    .await
    .unwrap();
    let (id, r): (String, String) = sqlx::query_as(
        "insert into tasks (board_id, ticket_no, title, status, priority, created_by, approved_at) \
         values ($1::uuid, $2, 'Review me', $3, 'medium', 'live-test', now()) \
         returning id::text, (select ticket_prefix from boards where id = $1::uuid) || '-' || $2",
    )
    .bind(board)
    .bind(no)
    .bind(status)
    .fetch_one(pg)
    .await
    .unwrap();
    (id, r)
}

async fn status_of(pg: &PgPool, id: &str) -> (String, bool) {
    sqlx::query_as("select status, approved_at is not null from tasks where id = $1::uuid")
        .bind(id)
        .fetch_one(pg)
        .await
        .unwrap()
}

async fn back_to_review(pg: &PgPool, id: &str) {
    sqlx::query("update tasks set status = 'quality_review' where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await
        .unwrap();
}

/// An org agent (no personal owner) with its own key, on the boards named.
async fn agent(state: &AppState, tag: &str, boards: &[&str]) -> (String, String) {
    let slug = format!(
        "trx{tag}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let model = format!("{slug}-engineering");
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'engineering', $2, 'Exit Live Agent') returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    for b in boards {
        sqlx::query("insert into board_agents (board_id, agent_model) values ($1::uuid, $2)")
            .bind(b)
            .bind(&model)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, &id)
        .await
        .expect("agent key mints");
    (model, key)
}

async fn as_agent(
    state: &AppState,
    key: &str,
    method: &str,
    path: &str,
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
    let res = routes::router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn conversation(pg: &PgPool, owner: &str, agent: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into conversations (user_id, agent_model) values ($1::uuid, $2) returning id::text",
    )
    .bind(owner)
    .bind(agent)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// A row in a conversation, as the chat door writes it. `author` is the
/// session that sent a user row; `status` is the reply's.
#[allow(clippy::too_many_arguments)] // one row's columns, each named
async fn row(
    pg: &PgPool,
    conv: &str,
    seq: i32,
    role: &str,
    author: Option<&str>,
    content: &str,
    status: &str,
    metadata: Value,
) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, content, status, author_user_id, metadata) \
         values ($1::uuid, $2, $3, $4, $5, $6::uuid, $7) returning id::text",
    )
    .bind(conv)
    .bind(seq)
    .bind(role)
    .bind(content)
    .bind(status)
    .bind(author)
    .bind(metadata)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// A conversation where `asker` just wrote `content` and the agent is
/// replying now. Returns (conversation id, the asker's message id).
async fn live_chat(
    pg: &PgPool,
    asker: &str,
    agent: &str,
    content: &str,
    drafted: bool,
) -> (String, String) {
    let conv = conversation(pg, asker, agent).await;
    row(
        pg,
        &conv,
        1,
        "user",
        Some(asker),
        "hi",
        "complete",
        json!({}),
    )
    .await;
    row(
        pg,
        &conv,
        2,
        "assistant",
        None,
        "hello",
        "complete",
        json!({}),
    )
    .await;
    let meta = if drafted {
        json!({ "drafted": true })
    } else {
        json!({})
    };
    let msg = row(pg, &conv, 3, "user", Some(asker), content, "complete", meta).await;
    row(pg, &conv, 4, "assistant", None, "", "streaming", json!({})).await;
    (conv, msg)
}

async fn activity(pg: &PgPool, id: &str) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "select actor, type, description from task_activity where task_id = $1::uuid order by created_at, id",
    )
    .bind(id)
    .fetch_all(pg)
    .await
    .unwrap()
}

async fn audit_after(pg: &PgPool, action: &str, id: &str) -> Vec<Value> {
    sqlx::query_scalar(
        "select after from audit_log where action = $1 and target_id = $2 order by created_at",
    )
    .bind(action)
    .bind(id)
    .fetch_all(pg)
    .await
    .unwrap()
}

fn put(id: &str) -> String {
    format!("/api/tasks/{id}")
}

fn err(body: &Value) -> String {
    body["error"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
#[ignore]
async fn an_agent_leaves_review_only_on_a_proven_ask_and_only_once() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "a").await;
    let (ada, _, ada_email) = member(&state, "a", "ada").await;
    let b = board(&pg, &ada, "Exit Board", "TRXA").await;
    let (model, key) = agent(&state, "a", &[&b]).await;
    let (id, tref) = ticket(&pg, &b, "quality_review").await;

    // No ask at all: today's rule, with the way through in the sentence.
    let (s, body) = as_agent(&state, &key, "PUT", &put(&id), json!({ "status": "inbox" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(
        err(&body).contains("conversationId") && err(&body).contains("card"),
        "{body}"
    );
    assert_eq!(status_of(&pg, &id).await.0, "quality_review");

    // A proven ask: Ada wrote the message the agent is answering, it names
    // the ticket, and she edits the board.
    let (conv, msg) = live_chat(
        &pg,
        &ada,
        &model,
        &format!(
            "{} needs revisions — move it back to the inbox",
            tref.to_lowercase()
        ),
        false,
    )
    .await;
    let (s, body) = as_agent(
        &state,
        &key,
        "PUT",
        &put(&id),
        json!({ "status": "inbox", "conversationId": conv }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (status, approved) = status_of(&pg, &id).await;
    assert_eq!(status, "inbox");
    assert!(
        !approved,
        "sending signed-off work back retires the sign-off"
    );
    let lines = activity(&pg, &id).await;
    let want = format!("moved to inbox (asked by {ada_email} in chat)");
    assert!(
        lines
            .iter()
            .any(|(actor, kind, text)| actor == &model && kind == "status" && text == &want),
        "{lines:?}"
    );
    let audits = audit_after(&pg, "task.review_exit", &id).await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0]["askedBy"]["messageId"], json!(msg));
    assert_eq!(audits[0]["askedBy"]["userId"], json!(ada));
    assert_eq!(audits[0]["askedIn"]["id"], json!(conv));
    assert_eq!(audits[0]["status"], "inbox");

    // ONE ASK, ONE EXIT: back in review, the same message cannot move it again.
    back_to_review(&pg, &id).await;
    let (s, body) = as_agent(
        &state,
        &key,
        "PUT",
        &put(&id),
        json!({ "status": "inbox", "conversationId": conv }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(
        err(&body).contains("already took this ticket out of review"),
        "{body}"
    );
    assert_eq!(status_of(&pg, &id).await.0, "quality_review");

    // Closing stays the person's, asked or not — a fresh ask, every terminal.
    for closed in ["done", "failed", "cancelled"] {
        let (conv, _) = live_chat(
            &pg,
            &ada,
            &model,
            &format!("{tref}: mark it {closed}"),
            false,
        )
        .await;
        let (s, body) = as_agent(
            &state,
            &key,
            "PUT",
            &put(&id),
            json!({ "status": closed, "conversationId": conv }),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{closed}: {body}");
        assert!(
            err(&body).contains("even when a person asks"),
            "{closed}: {body}"
        );
        assert_eq!(status_of(&pg, &id).await.0, "quality_review", "{closed}");
    }

    // A pickup column is the person's choice too: back to In progress (the
    // default board's agent-start column) on a fresh ask.
    let (conv, _) = live_chat(
        &pg,
        &ada,
        &model,
        &format!("please send {tref} back to work"),
        false,
    )
    .await;
    let (s, body) = as_agent(
        &state,
        &key,
        "PUT",
        &put(&id),
        json!({ "status": "in_progress", "conversationId": conv }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(status_of(&pg, &id).await.0, "in_progress");
    sweep(&pg, "a").await;
}

#[tokio::test]
#[ignore]
async fn an_ask_that_does_not_prove_leaves_the_ticket_in_review() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "n").await;
    let (ada, _, _) = member(&state, "n", "ada").await;
    let (vic, _, vic_email) = member(&state, "n", "vic").await;
    let b = board(&pg, &ada, "Exit Unproven", "TRXN").await;
    share(&pg, &b, &vic, "viewer").await;
    let (model, key) = agent(&state, "n", &[&b]).await;
    let (other, _) = agent(&state, "n", &[&b]).await;
    let (id, tref) = ticket(&pg, &b, "quality_review").await;
    let ask = format!("{tref} needs revisions, move it back");
    let attempt = |conv: String| {
        let state = state.clone();
        let key = key.clone();
        let id = id.clone();
        async move {
            as_agent(
                &state,
                &key,
                "PUT",
                &put(&id),
                json!({ "status": "inbox", "conversationId": conv }),
            )
            .await
        }
    };

    // Another agent's thread.
    let (conv, _) = live_chat(&pg, &ada, &other, &ask, false).await;
    let (s, body) = attempt(conv).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(err(&body).contains("names no thread of yours"), "{body}");

    // This agent's thread, but no reply streaming there now.
    let conv = conversation(&pg, &ada, &model).await;
    row(
        &pg,
        &conv,
        1,
        "user",
        Some(&ada),
        &ask,
        "complete",
        json!({}),
    )
    .await;
    row(
        &pg,
        &conv,
        2,
        "assistant",
        None,
        "done",
        "complete",
        json!({}),
    )
    .await;
    let (s, body) = attempt(conv).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(err(&body).contains("not replying in that thread"), "{body}");

    // A viewer's ask: she could not move it herself.
    let (conv, _) = live_chat(&pg, &vic, &model, &ask, false).await;
    let (s, body) = attempt(conv).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(
        err(&body).contains(&format!(
            "{vic_email} cannot edit every board this move touches"
        )),
        "{body}"
    );

    // The message does not name the ticket.
    let (conv, _) = live_chat(&pg, &ada, &model, "move it back please", false).await;
    let (s, body) = attempt(conv).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(err(&body).contains("does not name this ticket"), "{body}");

    // Words the agent offered and Ada only clicked.
    let (conv, _) = live_chat(&pg, &ada, &model, &ask, true).await;
    let (s, body) = attempt(conv).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(err(&body).contains("only clicked"), "{body}");

    // A cron's turn: the responsible user stamped on the REPLY, no person's
    // message in front of it.
    let conv = conversation(&pg, &ada, &model).await;
    row(
        &pg,
        &conv,
        1,
        "assistant",
        Some(&ada),
        &ask,
        "streaming",
        json!({}),
    )
    .await;
    let (s, body) = attempt(conv).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(err(&body).contains("no person behind it"), "{body}");

    // A string that is no thread id at all is the route's 400.
    let (s, _) = attempt("not-a-uuid".into()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    assert_eq!(status_of(&pg, &id).await.0, "quality_review");
    assert!(audit_after(&pg, "task.review_exit", &id).await.is_empty());
    sweep(&pg, "n").await;
}

#[tokio::test]
#[ignore]
async fn in_the_tickets_own_room_the_ask_need_not_name_it() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "r").await;
    let (ada, _, ada_email) = member(&state, "r", "ada").await;
    let b = board(&pg, &ada, "Exit Room", "TRXR").await;
    let (model, key) = agent(&state, "r", &[&b]).await;
    let (id, _) = ticket(&pg, &b, "quality_review").await;
    let (room,): (String,) = sqlx::query_as(
        "insert into channels (name, created_by, task_id) values ('trx-room', $1::uuid, $2::uuid) \
         returning id::text",
    )
    .bind(&ada)
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    for (seq, author_type, author, content, status) in [
        (
            1,
            "user",
            ada_email.as_str(),
            "@agent send this back to the inbox",
            "complete",
        ),
        (2, "agent", model.as_str(), "", "streaming"),
    ] {
        sqlx::query(
            "insert into channel_messages (channel_id, seq, author_type, author, content, status) \
             values ($1::uuid, $2, $3, $4, $5, $6)",
        )
        .bind(&room)
        .bind(seq)
        .bind(author_type)
        .bind(author)
        .bind(content)
        .bind(status)
        .execute(&pg)
        .await
        .unwrap();
    }
    let (s, body) = as_agent(
        &state,
        &key,
        "PUT",
        &put(&id),
        json!({ "status": "inbox", "conversationId": room }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let want = format!("moved to inbox (asked by {ada_email} in the ticket's discussion)");
    assert!(
        activity(&pg, &id).await.iter().any(|(_, _, t)| t == &want),
        "{:?}",
        activity(&pg, &id).await
    );
    sweep(&pg, "r").await;
}

#[tokio::test]
#[ignore]
async fn a_board_move_out_of_review_needs_the_asker_on_both_boards() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "m").await;
    let (ada, _, ada_email) = member(&state, "m", "ada").await;
    let (owner, _, _) = member(&state, "m", "own").await;
    let src = board(&pg, &ada, "Exit Move Src", "TRXS").await;
    let edits = board(&pg, &ada, "Exit Move Edit", "TRXE").await;
    let views = board(&pg, &owner, "Exit Move View", "TRXV").await;
    share(&pg, &views, &ada, "viewer").await;
    let (model, key) = agent(&state, "m", &[&src, &edits, &views]).await;
    let (id, tref) = ticket(&pg, &src, "quality_review").await;

    // The destination Ada only views: refused, nothing moved.
    let (conv, _) = live_chat(
        &pg,
        &ada,
        &model,
        &format!("move {tref} to the other board"),
        false,
    )
    .await;
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &format!("/api/tasks/{id}/move"),
        json!({ "boardId": views, "status": "inbox", "conversationId": conv }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(
        err(&body).contains("cannot edit every board this move touches"),
        "{body}"
    );

    // No ask: today's review stop.
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &format!("/api/tasks/{id}/move"),
        json!({ "boardId": edits }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(err(&body).contains("conversationId"), "{body}");

    // A destination she edits, on a fresh ask: the move, named on both lines
    // and in the audit row.
    let (conv, msg) = live_chat(
        &pg,
        &ada,
        &model,
        &format!("move {tref} to the edit board"),
        false,
    )
    .await;
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &format!("/api/tasks/{id}/move"),
        json!({ "boardId": edits, "status": "inbox", "conversationId": conv }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let audits = audit_after(&pg, "task.board_move", &id).await;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0]["askedBy"]["messageId"], json!(msg));
    assert_eq!(audits[0]["boardId"], json!(edits));
    let note = format!("asked by {ada_email} in chat");
    assert!(
        activity(&pg, &id)
            .await
            .iter()
            .any(|(actor, kind, t)| actor == &model && kind == "status" && t.contains(&note)),
        "{:?}",
        activity(&pg, &id).await
    );
    sweep(&pg, "m").await;
}

/// A refused move leaves a card; a board editor who is neither the agent's
/// owner nor an admin approves it, and the move is theirs.
async fn card(pg: &PgPool, kind: &str, agent: &str, payload: Value) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into chat_approvals (kind, summary, payload, external_id, agent_model, status) \
         values ($1, 'Move ticket', $2, $3, $4, 'pending') returning id::text",
    )
    .bind(kind)
    .bind(&payload)
    .bind(format!("live:{}", uuid::Uuid::new_v4()))
    .bind(agent)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

#[tokio::test]
#[ignore]
async fn a_board_editor_approves_the_card_and_a_viewer_cannot() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "c").await;
    let (ada, _, _) = member(&state, "c", "ada").await;
    let (eve, eve_cookie, eve_email) = member(&state, "c", "eve").await;
    let (vic, vic_cookie, _) = member(&state, "c", "vic").await;
    let b = board(&pg, &ada, "Exit Card", "TRXC").await;
    let other = board(&pg, &ada, "Exit Card Dest", "TRXD").await;
    share(&pg, &b, &eve, "editor").await;
    share(&pg, &other, &eve, "editor").await;
    share(&pg, &b, &vic, "viewer").await;
    let (model, _) = agent(&state, "c", &[&b, &other]).await;
    let (id, tref) = ticket(&pg, &b, "quality_review").await;

    // The card carries the REF, as the agent sent it.
    let triage = card(
        &pg,
        "ticket_move",
        &model,
        json!({ "taskId": tref, "status": "inbox" }),
    )
    .await;
    let approve = json!({ "decision": "approve" });
    let path = format!("/api/chat/chips/approvals/{triage}");
    let (s, body) = call_json(
        &state,
        "POST",
        &path,
        Some(&vic_cookie),
        Some(approve.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a viewer cannot approve: {body}");
    let (s, body) = call_json(
        &state,
        "POST",
        &path,
        Some(&eve_cookie),
        Some(approve.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "approved");
    assert_eq!(
        body["tools"],
        json!([]),
        "the approval IS the move; it unlocks nothing"
    );
    assert_eq!(status_of(&pg, &id).await.0, "inbox");
    let lines = activity(&pg, &id).await;
    assert!(
        lines.iter().any(|(actor, kind, t)| actor == &eve_email
            && kind == "status"
            && t.starts_with("moved to inbox (approved ")
            && t.ends_with("'s request)")),
        "{lines:?}"
    );

    // The board-move card runs move_task as the approving person.
    back_to_review(&pg, &id).await;
    let mv = card(
        &pg,
        "ticket_board_move",
        &model,
        json!({ "taskId": id, "boardId": other, "status": "inbox" }),
    )
    .await;
    let path = format!("/api/chat/chips/approvals/{mv}");
    let (s, body) = call_json(
        &state,
        "POST",
        &path,
        Some(&vic_cookie),
        Some(approve.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    let (s, body) = call_json(&state, "POST", &path, Some(&eve_cookie), Some(approve)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (board_now,): (String,) =
        sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(&id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(board_now, other);
    let audits = audit_after(&pg, "task.board_move", &id).await;
    assert_eq!(audits.len(), 1);
    let (actor,): (String,) = sqlx::query_as(
        "select actor from audit_log where action = 'task.board_move' and target_id = $1",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(actor, eve_email);
    sweep(&pg, "c").await;
}

// Live-DB + live-Redis proof that a malformed path id is a plain miss on
// every route that casts one with `$N::uuid`: it answers 404, never the 500
// the failed cast was, and never a body carrying the database's own error
// ("invalid input syntax for type uuid"), which some of these routes passed
// through as a 400.
//
// Each route below was found by driving every path-param route of the router
// with a malformed id against a running stack. The gate in front of the cast
// is `talaria_params::uuid_gate_404`, whose shape check is Postgres' own uuid
// input (so a well-formed id still reaches the read). A bad body still
// answers its 400 first where the route validates the body before its lookup,
// and a caller with no session hears what it hears for any other id.
//
// The 404 is the answer even where the route answers a well-formed absent id
// its own way (a 403 that hides whether the row exists, an empty list, an
// idempotent ok): no row can have a malformed id, so the 404 hides nothing.
//
// The caller is an admin, which passes every agent-manager and permission
// gate, so each request reaches the id. The channel routes get a channel the
// admin is a member of, so the gate under test is the message id's.
// House rule: #[ignore]d; CI's api-integration job runs it.
//
//   source ui/.env && cargo test -p talaria-api --test it malformed_path_ids:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const PREFIX: &str = "malformed-path-ids";

/// Spellings no `::uuid` cast takes: a word, a truncated id, and the urn
/// form (which `uuid::Uuid::parse_str` accepts and Postgres refuses).
const MALFORMED: [&str; 3] = [
    "not-a-uuid",
    "6113b5bc-6935-46e1-a67c-60bd58daf4f",
    "urn:uuid:6113b5bc-6935-46e1-a67c-60bd58daf4f0",
];

/// Each test sweeps only the rows its own `tag` made: the tests in a file run
/// concurrently, and one test's sweep deleting a sibling's admin mid-flight
/// failed that sibling's channel insert on the foreign key.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from channels where name like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// An admin with a session minted the way the auth routes mint one.
async fn minted_admin(state: &AppState, tag: &str) -> (String, String) {
    let sub = format!("{PREFIX}:{tag}:{}", uuid::Uuid::new_v4());
    let (id,): (String,) =
        sqlx::query_as("insert into users (sub, role) values ($1, 'admin') returning id::text")
            .bind(&sub)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let user = SessionUser {
        id: id.clone(),
        sub,
        email: None,
        name: None,
        picture: None,
        role: "admin".into(),
        provider: "password".into(),
    };
    let sid = create_session(state, &user)
        .await
        .expect("session mints — needs REDIS_URL");
    (id, format!("talaria_session={sid}"))
}

/// A channel with the admin in it, so its message routes reach the message
/// id rather than refusing a non-member first.
async fn channel(pg: &PgPool, member: &str, tag: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into channels (name, kind) values ($1, 'channel') returning id::text",
    )
    .bind(format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4()))
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query("insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&id)
        .bind(member)
        .execute(pg)
        .await
        .unwrap();
    id
}

/// Every route that answered 500 (or the database's error text) for a
/// malformed id, with `{bad}` where the id goes and a body that passes the
/// route's own validation.
fn routes(channel: &str) -> Vec<(&'static str, String, Option<Value>)> {
    let ch = channel;
    let job = "0123456789ab";
    vec![
        // comms
        ("GET", "/api/conversations/{bad}".into(), None),
        (
            "POST",
            "/api/conversations/{bad}/read".into(),
            Some(json!({})),
        ),
        ("GET", "/api/inbox/focus/conversations/{bad}".into(), None),
        (
            "DELETE",
            "/api/inbox/focus/conversations/{bad}".into(),
            None,
        ),
        (
            "PATCH",
            format!("/api/channels/{ch}/messages/{{bad}}"),
            Some(json!({ "content": "edited" })),
        ),
        (
            "DELETE",
            format!("/api/channels/{ch}/messages/{{bad}}"),
            None,
        ),
        (
            "POST",
            "/api/chat/chips/approvals/{bad}".into(),
            Some(json!({ "decision": "approve" })),
        ),
        // knowledge and files
        ("DELETE", "/api/kb/spaces/{bad}".into(), None),
        ("GET", "/api/kb/docs/{bad}/backlinks".into(), None),
        (
            "PATCH",
            "/api/kb/comments/{bad}".into(),
            Some(json!({ "resolved": true })),
        ),
        ("DELETE", "/api/kb/comments/{bad}".into(), None),
        ("POST", "/api/artifacts/{bad}/links".into(), Some(json!({}))),
        (
            "DELETE",
            "/api/artifacts/{bad}/links".into(),
            Some(json!({})),
        ),
        ("POST", "/api/artifacts/{bad}/duplicate".into(), None),
        ("GET", "/api/artifacts/{bad}/comments".into(), None),
        (
            "POST",
            "/api/artifacts/{bad}/comments".into(),
            Some(json!({ "content": "a comment" })),
        ),
        ("POST", "/api/artifacts/{bad}/export/google".into(), None),
        ("POST", "/api/artifact-folders/{bad}/duplicate".into(), None),
        ("GET", "/api/artifact-folders/{bad}".into(), None),
        (
            "PUT",
            "/api/artifact-folders/{bad}".into(),
            Some(json!({ "name": "renamed" })),
        ),
        ("DELETE", "/api/artifact-folders/{bad}".into(), None),
        ("GET", "/api/uploads/{bad}".into(), None),
        ("GET", "/api/memory/{bad}".into(), None),
        (
            "PUT",
            "/api/memory/{bad}".into(),
            Some(json!({ "content": "remember" })),
        ),
        (
            "PUT",
            "/api/templates/{bad}".into(),
            Some(json!({ "name": "renamed" })),
        ),
        ("DELETE", "/api/templates/{bad}".into(), None),
        // fleet
        ("GET", "/api/fleet/agents/{bad}/crons".into(), None),
        (
            "POST",
            "/api/fleet/agents/{bad}/crons".into(),
            Some(json!({ "name": "n", "schedule": "0 9 * * *", "prompt": "p" })),
        ),
        (
            "PUT",
            format!("/api/fleet/agents/{{bad}}/crons/{job}"),
            Some(json!({ "name": "n" })),
        ),
        (
            "POST",
            format!("/api/fleet/agents/{{bad}}/crons/{job}"),
            Some(json!({ "action": "pause" })),
        ),
        (
            "DELETE",
            format!("/api/fleet/agents/{{bad}}/crons/{job}"),
            None,
        ),
        ("GET", "/api/fleet/agents/{bad}/secrets".into(), None),
        (
            "PUT",
            "/api/fleet/agents/{bad}/secrets".into(),
            Some(json!({ "name": "MALFORMED_PROBE", "value": "v" })),
        ),
        (
            "DELETE",
            "/api/fleet/agents/{bad}/secrets".into(),
            Some(json!({ "name": "MALFORMED_PROBE" })),
        ),
        ("GET", "/api/runs/{bad}/events".into(), None),
        ("GET", "/api/runs/{bad}/watch".into(), None),
        ("GET", "/api/runs/{bad}/transcript".into(), None),
        ("GET", "/api/fleet/defs/{bad}/google".into(), None),
        (
            "PUT",
            "/api/fleet/defs/{bad}/google".into(),
            Some(json!({ "principalKind": "org" })),
        ),
        ("DELETE", "/api/fleet/defs/{bad}/google".into(), None),
        ("GET", "/api/fleet/defs/{bad}/google/connect".into(), None),
        ("GET", "/api/fleet/defs/{bad}/versions".into(), None),
        (
            "POST",
            "/api/fleet/defs/{bad}/versions".into(),
            Some(json!({ "revertTo": 1 })),
        ),
        (
            "PUT",
            "/api/gaps/{bad}".into(),
            Some(json!({ "status": "open" })),
        ),
        ("GET", "/api/fleet/defs/{bad}/mcp".into(), None),
        (
            "POST",
            "/api/fleet/defs/{bad}/mcp".into(),
            Some(json!({ "assign": [], "apply": false })),
        ),
        (
            "PATCH",
            "/api/fleet/defs/{bad}".into(),
            Some(json!({ "role": "probe" })),
        ),
        (
            "POST",
            "/api/fleet/defs/{bad}/edit".into(),
            Some(json!({
                "soul": "probe",
                "main": { "endpoint": "probe", "model": "probe" },
                "aliases": [],
                "fallbacks": [],
            })),
        ),
        ("POST", "/api/fleet/defs/{bad}/rotate-key".into(), None),
        (
            "POST",
            "/api/fleet/agents/{bad}/control".into(),
            Some(json!({ "action": "stop" })),
        ),
        // integrations
        (
            "POST",
            "/api/integrations/google/pending/{bad}".into(),
            Some(json!({ "decision": "reject" })),
        ),
    ]
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_malformed_path_id_is_a_miss_on_every_route_that_casts_it() {
    let state = live_app_state().await;
    cleanup(&state.pg, "miss").await;
    let (admin, cookie) = minted_admin(&state, "miss").await;
    let ch = channel(&state.pg, &admin, "miss").await;

    let mut wrong = Vec::new();
    for (method, path, body) in routes(&ch) {
        for bad in MALFORMED {
            let uri = path.replace("{bad}", bad);
            let (status, answer) =
                call_json(&state, method, &uri, Some(&cookie), body.clone()).await;
            let text = answer.to_string().to_ascii_lowercase();
            if status != StatusCode::NOT_FOUND
                || text.contains("database")
                || text.contains("syntax")
                || text.contains("uuid")
            {
                wrong.push(format!("{method} {uri} → {status} {answer}"));
            }
        }
    }
    cleanup(&state.pg, "miss").await;
    assert!(
        wrong.is_empty(),
        "these answered something other than a plain 404:\n{}",
        wrong.join("\n")
    );
}

/// The gate passes what the cast takes: a well-formed id that names nothing
/// still reaches the read and answers the route's ordinary miss, upper case
/// and braces included (Postgres' uuid input takes both).
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_well_formed_id_still_reaches_the_read() {
    let state = live_app_state().await;
    cleanup(&state.pg, "read").await;
    let (_admin, cookie) = minted_admin(&state, "read").await;
    let absent = uuid::Uuid::new_v4().to_string();
    for id in [
        absent.clone(),
        absent.to_ascii_uppercase(),
        // `{…}`, percent-encoded as a URL carries it; the router decodes it.
        format!("%7B{absent}%7D"),
        absent.replace('-', ""),
    ] {
        for path in [
            format!("/api/fleet/defs/{id}/versions"),
            format!("/api/templates/{id}"),
            format!("/api/artifact-folders/{id}"),
        ] {
            let method = if path.starts_with("/api/templates") {
                "DELETE"
            } else {
                "GET"
            };
            let (status, answer) = call_json(&state, method, &path, Some(&cookie), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path} → {answer}");
        }
    }
    cleanup(&state.pg, "read").await;
}

/// Who may ask is answered before the id is: with no session, a malformed id
/// gets exactly what a well-formed absent id gets — the 401 on most routes,
/// and the 404 where a route reads its row before it asks who is calling
/// (`DELETE /api/kb/spaces/{id}`, `POST /api/artifacts/{id}/export/google`).
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn who_may_ask_is_answered_before_the_id() {
    let state = live_app_state().await;
    let absent = uuid::Uuid::new_v4().to_string();
    // No session reaches a channel's members, so any channel id will do.
    let ch = uuid::Uuid::new_v4().to_string();

    let mut wrong = Vec::new();
    for (method, path, body) in routes(&ch) {
        let (want, _) = call_json(
            &state,
            method,
            &path.replace("{bad}", &absent),
            None,
            body.clone(),
        )
        .await;
        for bad in MALFORMED {
            let uri = path.replace("{bad}", bad);
            let (status, answer) = call_json(&state, method, &uri, None, body.clone()).await;
            if status != want {
                wrong.push(format!(
                    "{method} {uri} → {status} {answer}; a well-formed absent id → {want}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "these answered a malformed id before asking who is calling:\n{}",
        wrong.join("\n")
    );
}

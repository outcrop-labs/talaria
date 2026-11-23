// POST /api/artifacts/{id}/pull/google answers to the PUT's rule, not the
// read rule (talaria_routes_knowledge::files::artifacts_id_pull_google).
//
// The incident it closes: the route checked `can_read`, so a person who could
// only read a document — the editor hides Pull from Google from them — could
// still overwrite its title and body by calling the route. A pull replaces
// the body, so it is an edit: the owner, an org edit policy, or an editor
// grant. These drive the REAL router with minted sessions. Nothing here
// reaches Google: a reader is refused before the token read, and an editor
// with no connected Google account is answered 409 `not_connected` — which is
// how the test sees that the edit gate let them through.
//
//   source ui/.env && cargo test -p talaria-api --test it artifacts_pull_google_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "artifacts-pull-google-live";

async fn sweep(pg: &PgPool, who: &str, artifacts: &[&str]) {
    for id in artifacts {
        let _ = sqlx::query(
            "delete from kb_editors where item_type = 'artifact' and item_id = $1::uuid",
        )
        .bind(id)
        .execute(pg)
        .await;
        let _ = sqlx::query("delete from artifacts where id = $1::uuid")
            .bind(id)
            .execute(pg)
            .await;
        let _ =
            sqlx::query("delete from internal_versions where kind = 'artifact' and owner_key = $1")
                .bind(id)
                .execute(pg)
                .await;
    }
    let _ = sqlx::query("delete from users where sub like $1")
        .bind(format!("{who}:%"))
        .execute(pg)
        .await;
}

/// A document the owner made through the route, then marked as exported to a
/// Google Doc (the export itself needs Google; the column is what pull reads).
async fn exported_doc(
    state: &talaria_state::AppState,
    owner_cookie: &str,
    visibility: &str,
    edit_policy: &str,
) -> String {
    let (s, created) = call_json(
        state,
        "POST",
        "/api/artifacts",
        Some(owner_cookie),
        Some(json!({ "kind": "doc", "title": "Plan", "body": "the owner's text", "visibility": visibility })),
    )
    .await;
    assert!(s.is_success(), "create: {s} {created}");
    let id = created["artifact"]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    let (s, shared) = call_json(
        state,
        "PUT",
        &format!("/api/artifacts/{id}"),
        Some(owner_cookie),
        Some(json!({ "visibility": visibility, "editPolicy": edit_policy })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{shared}");
    sqlx::query(
        "update artifacts set google_file_id = '1pullGoogleLiveDoc', \
         google_file_url = 'https://docs.google.com/document/d/1pullGoogleLiveDoc/edit' \
         where id = $1::uuid",
    )
    .bind(&id)
    .execute(&state.pg)
    .await
    .unwrap();
    id
}

async fn stored(pg: &PgPool, id: &str) -> (String, String, Option<String>) {
    sqlx::query_as("select title, body, google_file_id from artifacts where id = $1::uuid")
        .bind(id)
        .fetch_one(pg)
        .await
        .unwrap()
}

async fn pull(state: &talaria_state::AppState, cookie: &str, id: &str) -> (StatusCode, Value) {
    call_json(
        state,
        "POST",
        &format!("/api/artifacts/{id}/pull/google"),
        Some(cookie),
        None,
    )
    .await
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL) and redis (REDIS_URL)"]
async fn a_reader_who_may_not_edit_cannot_pull_over_the_body() {
    let state = live_app_state().await;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let who = format!("{PREFIX}-{tag}");
    let (_, owner) =
        minted_member_with_email(&state, &who, &format!("owner-{tag}@example.com")).await;
    let (_, reader) =
        minted_member_with_email(&state, &who, &format!("reader-{tag}@example.com")).await;
    // Readable by the org, editable by its owner only.
    let id = exported_doc(&state, &owner, "org", "owner").await;

    let (s, read) = call_json(
        &state,
        "GET",
        &format!("/api/artifacts/{id}"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "the reader can read it: {read}");
    assert_eq!(read["artifact"]["canEdit"], json!(false), "{read}");

    let (s, refused) = pull(&state, &reader, &id).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "a reader may not overwrite the body: {refused}"
    );
    assert_eq!(refused["error"], json!("forbidden"), "{refused}");
    assert_eq!(
        stored(&state.pg, &id).await,
        (
            "Plan".to_string(),
            "the owner's text".to_string(),
            Some("1pullGoogleLiveDoc".to_string())
        )
    );

    // The owner passes the edit gate and reaches the Google step: a fresh
    // account has no Google connection, so it is the 409 that says so.
    let (s, owner_pull) = pull(&state, &owner, &id).await;
    assert_eq!(s, StatusCode::CONFLICT, "{owner_pull}");
    assert_eq!(owner_pull["error"], json!("not_connected"), "{owner_pull}");

    sweep(&state.pg, &who, &[&id]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL) and redis (REDIS_URL)"]
async fn whoever_the_put_lets_edit_may_pull_and_nobody_else() {
    let state = live_app_state().await;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let who = format!("{PREFIX}-{tag}");
    let (_, owner) =
        minted_member_with_email(&state, &who, &format!("owner-{tag}@example.com")).await;
    let (member_id, member) =
        minted_member_with_email(&state, &who, &format!("member-{tag}@example.com")).await;

    // An org edit policy makes every member an editor: the pull is theirs.
    let org = exported_doc(&state, &owner, "org", "org").await;
    let (s, body) = pull(&state, &member, &org).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "an org editor passes the gate: {body}"
    );
    assert_eq!(body["error"], json!("not_connected"), "{body}");

    // A private document they cannot even read: refused, as before.
    let private = exported_doc(&state, &owner, "private", "owner").await;
    let (s, body) = pull(&state, &member, &private).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");

    // A viewer grant lets them read it — still not pull over it.
    sqlx::query(
        "insert into kb_editors (item_type, item_id, principal_type, principal_id, role) \
         values ('artifact', $1::uuid, 'user', $2, 'viewer')",
    )
    .bind(&private)
    .bind(&member_id)
    .execute(&state.pg)
    .await
    .unwrap();
    let (s, read) = call_json(
        &state,
        "GET",
        &format!("/api/artifacts/{private}"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "a viewer grant reads it: {read}");
    let (s, body) = pull(&state, &member, &private).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "a viewer grant is not an edit: {body}"
    );
    assert_eq!(stored(&state.pg, &private).await.1, "the owner's text");

    // An editor grant is an edit: the gate lets them through.
    sqlx::query(
        "update kb_editors set role = 'editor' \
         where item_type = 'artifact' and item_id = $1::uuid and principal_id = $2",
    )
    .bind(&private)
    .bind(&member_id)
    .execute(&state.pg)
    .await
    .unwrap();
    let (s, body) = pull(&state, &member, &private).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "an editor grant passes the gate: {body}"
    );
    assert_eq!(body["error"], json!("not_connected"), "{body}");

    sweep(&state.pg, &who, &[&org, &private]).await;
}

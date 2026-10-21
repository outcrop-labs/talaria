// Live-DB + live-Redis proof for the blueprint library — /api/blueprints and
// /api/blueprints/{id} driven through the real router with real sessions.
//
// WHY ONLY A REAL DATABASE PROVES IT. Every claim here is a WHERE clause, a
// null, or a permission read. That a personal blueprint is invisible to
// everybody else is `owner_user_id = $1 or owner_user_id is null` and nothing
// else; that somebody else's row answers 404 rather than 403 is the shape of
// the read the handler does BEFORE it decides anything; that a shared
// blueprint needs `templates.manage` is `has_perm` against real
// `user_permissions` rows and the org defaults. Mocking any of the three would
// be re-asserting the SQL the handler already contains.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it blueprints_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "blueprints-live";

/// A throwaway person for ONE test. The binary runs its cases concurrently and
/// the list below is "mine plus the org's", so every test needs a person whose
/// personal rows are only its own — hence a uuid per person.
async fn member(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@{PREFIX}-{tag}.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

/// The same, holding `templates.manage`. The permission is INSERTED rather
/// than inherited: it is not a member default today, and a test that leaned on
/// an admin role would prove the admin bypass instead of the permission.
async fn sharer(state: &AppState, tag: &str) -> (String, String) {
    let (id, cookie) = member(state, tag).await;
    sqlx::query(
        "insert into user_permissions (user_id, perm, allowed) \
         values ($1::uuid, 'templates.manage', true) \
         on conflict (user_id, perm) do update set allowed = true",
    )
    .bind(&id)
    .execute(&state.pg)
    .await
    .unwrap();
    (id, cookie)
}

/// A name no other row carries — the list reads org-wide shared rows too, so
/// every assertion below filters on a name it minted itself. The uuid rides at
/// the END so the leading word still decides `order by name asc`.
fn named(word: &str) -> String {
    format!("{word} {}", uuid::Uuid::new_v4())
}

async fn create(state: &AppState, cookie: &str, body: Value) -> (StatusCode, Value) {
    call_json(state, "POST", "/api/blueprints", Some(cookie), Some(body)).await
}

/// One created blueprint, or a failed assertion naming the answer.
async fn created(state: &AppState, cookie: &str, body: Value) -> Value {
    let (status, answer) = create(state, cookie, body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["blueprint"].clone()
}

fn id_of(blueprint: &Value) -> String {
    blueprint["id"]
        .as_str()
        .expect("a blueprint carries an id")
        .to_string()
}

/// This caller's list, as the client reads it.
async fn list(state: &AppState, cookie: &str) -> Vec<Value> {
    let (status, body) = call_json(state, "GET", "/api/blueprints", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["blueprints"]
        .as_array()
        .expect("the list answers an array")
        .clone()
}

fn names(rows: &[Value]) -> Vec<&str> {
    rows.iter()
        .map(|r| r["name"].as_str().expect("every row carries a name"))
        .collect()
}

fn row_with<'a>(rows: &'a [Value], id: &str) -> Option<&'a Value> {
    rows.iter().find(|r| r["id"].as_str() == Some(id))
}

/// A personal blueprint: anybody may make one, it is the maker's, and the
/// camelCase wire shape is what the picker reads. Plus the only ordering the
/// list has — `sort` then `name` — which with the default sort is the name.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_member_creates_a_personal_blueprint_and_lists_it() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state, "own").await;

    let zeta = named("zeta");
    let alpha = named("alpha");
    // Created out of order on purpose: the list's order is the query's, not
    // the insert's.
    let first = created(
        &state,
        &cookie,
        json!({ "name": &zeta, "body": "ship the weekly rollup" }),
    )
    .await;
    let second = created(&state, &cookie, json!({ "name": &alpha, "body": "" })).await;

    assert_eq!(first["name"], json!(zeta), "{first}");
    assert_eq!(first["body"], json!("ship the weekly rollup"), "{first}");
    assert_eq!(first["shared"], json!(false), "{first}");
    assert_eq!(
        first["ownerUserId"],
        json!(me),
        "a personal blueprint is the maker's: {first}"
    );
    // The defaults ride the wire, so a client never has to guess them.
    assert_eq!(first["surface"], json!("work"), "{first}");
    assert_eq!(first["sort"], json!(0), "{first}");
    assert!(
        first["createdAt"].as_str().is_some_and(|s| s.contains('T')),
        "timestamps are ISO strings: {first}"
    );
    assert!(
        first["updatedAt"].as_str().is_some_and(|s| s.contains('T')),
        "timestamps are ISO strings: {first}"
    );
    // An empty body is a legal blueprint — a name now, the text later.
    assert_eq!(second["body"], json!(""), "{second}");

    let rows = list(&state, &cookie).await;
    let mine = names(&rows);
    let at = |name: &str| {
        mine.iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("{name} is in the maker's list: {mine:?}"))
    };
    assert!(at(&alpha) < at(&zeta), "sort asc, then name asc: {mine:?}");

    // The bounds, each a 400 the client can read.
    for bad in [
        json!({ "name": "   ", "body": "x" }),
        json!({ "name": "x".repeat(121), "body": "x" }),
        json!({ "name": "fine", "body": "x".repeat(20_001) }),
        json!({ "body": "a blueprint with no name" }),
    ] {
        let (status, answer) = create(&state, &cookie, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} → {answer}");
    }
    // 120 is the edge, and the trim runs before the bound: 120 characters
    // padded with spaces passes.
    let long = format!("  {}  ", "x".repeat(120));
    let edge = created(&state, &cookie, json!({ "name": long, "body": "" })).await;
    assert_eq!(edge["name"], json!("x".repeat(120)), "{edge}");
}

/// THE POINT OF THE NULLABLE OWNER: a personal blueprint belongs to one
/// person. Nobody else lists it, and to anybody else its id does not exist —
/// the refusal is 404 and not 403, so probing an id learns nothing.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn another_members_personal_blueprint_does_not_exist() {
    let state = live_app_state().await;
    let (_, mine) = member(&state, "private-owner").await;
    let (_, theirs) = member(&state, "private-stranger").await;

    let blueprint = created(
        &state,
        &mine,
        json!({ "name": named("private"), "body": "my own words" }),
    )
    .await;
    let id = id_of(&blueprint);
    let path = format!("/api/blueprints/{id}");

    let rows = list(&state, &theirs).await;
    assert!(
        row_with(&rows, &id).is_none(),
        "a stranger's list carries only their own and the org's: {rows:?}"
    );

    let (status, body) = call_json(
        &state,
        "PUT",
        &path,
        Some(&theirs),
        Some(json!({ "name": named("stolen") })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a row you cannot see does not exist: {body}"
    );
    let (status, body) = call_json(&state, "DELETE", &path, Some(&theirs), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // And it is still there, unedited, for the person whose it is.
    let (status, body) = call_json(
        &state,
        "PUT",
        &path,
        Some(&mine),
        Some(json!({ "body": "my own words, edited" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["blueprint"]["name"], blueprint["name"], "{body}");
    assert_eq!(
        body["blueprint"]["body"],
        json!("my own words, edited"),
        "{body}"
    );

    // An id that was never a blueprint is the same answer as somebody else's.
    let ghost = format!("/api/blueprints/{}", uuid::Uuid::new_v4());
    let (status, body) = call_json(&state, "DELETE", &ghost, Some(&mine), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// The org's own: owner null, readable by every member, and writable only with
/// `templates.manage`. The refusal here IS a 403 — everybody can see the row,
/// and what they are missing is a permission.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_shared_blueprint_needs_the_permission() {
    let state = live_app_state().await;
    let (_, plain) = member(&state, "shared-plain").await;
    let (_, allowed) = sharer(&state, "shared-allowed").await;

    // Without the permission, sharing is refused — and nothing is written.
    // Proven by the NAME and not by a row count: this database is shared with
    // whoever else is running, and a count is a race.
    let refused = named("refused");
    let (status, body) = create(
        &state,
        &plain,
        json!({ "name": &refused, "body": "x", "shared": true }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let rows = list(&state, &plain).await;
    assert!(
        !names(&rows).contains(&refused.as_str()),
        "a refused create writes nothing: {rows:?}"
    );
    // The same person may still keep their own.
    let own = created(
        &state,
        &plain,
        json!({ "name": named("personal"), "body": "x", "shared": false }),
    )
    .await;
    assert_eq!(own["shared"], json!(false), "{own}");

    // With it, the blueprint is the org's: no owner.
    let shared = created(
        &state,
        &allowed,
        json!({ "name": named("shared"), "body": "the org's ask", "shared": true }),
    )
    .await;
    let id = id_of(&shared);
    assert_eq!(shared["shared"], json!(true), "{shared}");
    assert_eq!(
        shared["ownerUserId"],
        Value::Null,
        "shared means no owner: {shared}"
    );

    // Every member reads it…
    let rows = list(&state, &plain).await;
    let row = row_with(&rows, &id).expect("a shared blueprint is in everybody's list");
    assert_eq!(row["shared"], json!(true), "{row}");
    assert_eq!(row["body"], json!("the org's ask"), "{row}");

    // …and cannot change or delete it.
    let path = format!("/api/blueprints/{id}");
    let (status, body) = call_json(
        &state,
        "PUT",
        &path,
        Some(&plain),
        Some(json!({ "body": "edited by somebody who may not" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a row everybody can see refuses with 403, not 404: {body}"
    );
    let (status, body) = call_json(&state, "DELETE", &path, Some(&plain), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Sharing one that was personal crosses the same line, so it asks the same
    // permission.
    let own_path = format!("/api/blueprints/{}", id_of(&own));
    let (status, body) = call_json(
        &state,
        "PUT",
        &own_path,
        Some(&plain),
        Some(json!({ "shared": true })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // The holder edits it, and the edit is what every member then reads.
    let (status, body) = call_json(
        &state,
        "PUT",
        &path,
        Some(&allowed),
        Some(json!({ "body": "the org's ask, sharpened" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["blueprint"]["shared"], json!(true), "{body}");
    let rows = list(&state, &plain).await;
    let row = row_with(&rows, &id).expect("still everybody's");
    assert_eq!(row["body"], json!("the org's ask, sharpened"), "{row}");
}

/// Delete removes the row: it leaves the list, and the id stops existing for
/// the person who owned it.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn delete_removes_a_blueprint() {
    let state = live_app_state().await;
    let (_, cookie) = member(&state, "delete").await;
    let blueprint = created(
        &state,
        &cookie,
        json!({ "name": named("doomed"), "body": "retired" }),
    )
    .await;
    let id = id_of(&blueprint);
    let path = format!("/api/blueprints/{id}");

    assert!(
        row_with(&list(&state, &cookie).await, &id).is_some(),
        "it is there before the delete"
    );
    let (status, body) = call_json(&state, "DELETE", &path, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "{body}");

    assert!(
        row_with(&list(&state, &cookie).await, &id).is_none(),
        "and gone after it"
    );
    let (status, body) = call_json(&state, "DELETE", &path, Some(&cookie), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a second delete has nothing to remove: {body}"
    );
    let (count,): (i64,) = sqlx::query_as("select count(*) from blueprints where id = $1::uuid")
        .bind(&id)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "the row is gone from the table, not just the list"
    );
}

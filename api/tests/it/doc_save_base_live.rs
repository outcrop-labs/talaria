// PUT /api/artifacts/{id} and PUT /api/kb/docs/{id} with `baseUpdatedAt` —
// the editor's save, made atomic with its version check
// (`save_artifact_content_if_unchanged` / `save_doc_content_if_unchanged`).
//
// The incident: a person had a document open in Edit, an agent's
// update_document / edit_kb_doc landed, and the person's next autosave
// replaced the agent's edit with nothing on screen to say so. The editor now
// names the version its buffer started from; these drive the REAL router
// with a minted session and pin what the route answers: a stale base is
// refused with the row (409) and the agent's text stays; a save that would
// change nothing that moved lands; a rename on a moved row is refused; no
// base keeps the old replace; a malformed base is a 400; a caller who may
// not edit gets 403 before any of that (a 409 would hand them the row); and
// the GET's `canEdit` says the same as the PUT. The agent's save is written
// straight to the row — the route under test is the person's.
//
//   DATABASE_URL=… REDIS_URL=… cargo test -p talaria-api --test it -- --ignored doc_save_base_live::

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "doc-save-base";

async fn sweep(pg: &PgPool, who: &str, artifacts: &[&str], docs: &[&str], spaces: &[&str]) {
    for id in artifacts {
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
    for id in docs {
        let _ = sqlx::query("delete from kb_docs where id = $1::uuid")
            .bind(id)
            .execute(pg)
            .await;
        let _ =
            sqlx::query("delete from internal_versions where kind = 'kb-doc' and owner_key = $1")
                .bind(id)
                .execute(pg)
                .await;
    }
    for id in spaces {
        let _ = sqlx::query("delete from kb_spaces where id = $1::uuid")
            .bind(id)
            .execute(pg)
            .await;
    }
    let _ = sqlx::query("delete from users where sub like $1")
        .bind(format!("{who}:%"))
        .execute(pg)
        .await;
}

/// An admin with a session minted the way the auth routes mint one — an
/// org-visible space needs `kb.official`, which members do not hold.
async fn minted_admin(state: &talaria_state::AppState, who: &str, email: &str) -> String {
    let sub = format!("{who}:{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, role) values ($1, $2, 'admin') returning id::text",
    )
    .bind(&sub)
    .bind(email)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let user = talaria_api::session::SessionUser {
        id,
        sub,
        email: Some(email.to_string()),
        name: None,
        picture: None,
        role: "admin".into(),
        provider: "password".into(),
    };
    let sid = talaria_api::session::create_session(state, &user)
        .await
        .expect("session mints");
    format!("talaria_session={sid}")
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_str()
        .unwrap_or_else(|| panic!("{path:?} is not a string in {v}"))
}

/// The two tables under test. Static SQL per table: sqlx takes no
/// formatted query strings.
#[derive(Clone, Copy)]
enum Table {
    Artifacts,
    KbDocs,
}

/// An agent's save landing under the editor: a new body, a moved version.
async fn agent_writes(pg: &PgPool, table: Table, id: &str, body: &str) {
    let sql = match table {
        Table::Artifacts => {
            "update artifacts set body = $2, updated_by = 'docbot-research', \
             updated_at = updated_at + interval '1 second' where id = $1::uuid"
        }
        Table::KbDocs => {
            "update kb_docs set body = $2, updated_by = 'docbot-research', \
             updated_at = updated_at + interval '1 second' where id = $1::uuid"
        }
    };
    sqlx::query(sql)
        .bind(id)
        .bind(body)
        .execute(pg)
        .await
        .unwrap();
}

async fn stored_body(pg: &PgPool, table: Table, id: &str) -> (String, String) {
    let sql = match table {
        Table::Artifacts => "select body, title from artifacts where id = $1::uuid",
        Table::KbDocs => "select body, title from kb_docs where id = $1::uuid",
    };
    sqlx::query_as(sql).bind(id).fetch_one(pg).await.unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL) and redis (REDIS_URL)"]
async fn artifact_put_refuses_a_stale_base_with_the_row_and_keeps_the_agents_text() {
    let state = live_app_state().await;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    // Per test: the tests run in parallel, and a sweep must not take another
    // test's people with it.
    let who = format!("{PREFIX}-{tag}");
    let (_owner, cookie) =
        minted_member_with_email(&state, &who, &format!("owner-{tag}@example.com")).await;
    let (s, created) = call_json(
        &state,
        "POST",
        "/api/artifacts",
        Some(&cookie),
        Some(json!({ "kind": "doc", "title": "Plan", "body": "v1", "visibility": "private" })),
    )
    .await;
    assert!(s.is_success(), "create: {s} {created}");
    let id = str_at(&created, &["artifact", "id"]).to_string();
    let path = format!("/api/artifacts/{id}");
    let t0 = str_at(&created, &["artifact", "updatedAt"]).to_string();

    // The editor's own save, on the version it started from, lands.
    let (s, saved) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "body": "mine", "baseUpdatedAt": t0 })),
    )
    .await;
    assert_eq!(s, 200, "{saved}");
    let t1 = str_at(&saved, &["artifact", "updatedAt"]).to_string();

    agent_writes(&state.pg, Table::Artifacts, &id, "agent text").await;

    // The stale buffer is refused with the row as it now is; nothing written.
    let (s, refused) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "body": "my stale buffer", "baseUpdatedAt": t1 })),
    )
    .await;
    assert_eq!(s, 409, "{refused}");
    assert_eq!(refused["conflict"], json!(true));
    assert_eq!(str_at(&refused, &["artifact", "body"]), "agent text");
    assert_eq!(
        stored_body(&state.pg, Table::Artifacts, &id).await.0,
        "agent text"
    );

    // A rename riding the agent's own body is still a change over a moved row.
    let (s, renamed) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "title": "Mine", "body": "agent text", "baseUpdatedAt": t1 })),
    )
    .await;
    assert_eq!(s, 409, "{renamed}");
    assert_eq!(
        stored_body(&state.pg, Table::Artifacts, &id).await.1,
        "Plan"
    );

    // Nothing to overwrite: the row's own body on a stale base lands.
    let (s, same) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "body": "agent text", "baseUpdatedAt": t1 })),
    )
    .await;
    assert_eq!(s, 200, "{same}");

    // The fresh version carries title and body together, atomically.
    let t2 = str_at(&same, &["artifact", "updatedAt"]).to_string();
    let (s, both) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "title": "Plan, revised", "body": "agent text, then mine", "baseUpdatedAt": t2 })),
    )
    .await;
    assert_eq!(s, 200, "{both}");
    assert_eq!(
        stored_body(&state.pg, Table::Artifacts, &id).await,
        (
            "agent text, then mine".to_string(),
            "Plan, revised".to_string()
        )
    );

    // No base: the old replace (tools, restores, "keep mine").
    let (s, _) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "body": "forced" })),
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(
        stored_body(&state.pg, Table::Artifacts, &id).await.0,
        "forced"
    );

    // A base that is not a timestamp is the caller's mistake, said at 400.
    let (s, bad) = call_json(
        &state,
        "PUT",
        &path,
        Some(&cookie),
        Some(json!({ "body": "x", "baseUpdatedAt": "yesterday" })),
    )
    .await;
    assert_eq!(s, 400, "{bad}");

    sweep(&state.pg, &who, &[&id], &[], &[]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL) and redis (REDIS_URL)"]
async fn artifact_non_editor_with_a_stale_base_gets_403_and_reads_can_edit_false() {
    let state = live_app_state().await;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    // Per test: the tests run in parallel, and a sweep must not take another
    // test's people with it.
    let who = format!("{PREFIX}-{tag}");
    let (_owner, owner) =
        minted_member_with_email(&state, &who, &format!("owner-{tag}@example.com")).await;
    let (_other, other) =
        minted_member_with_email(&state, &who, &format!("other-{tag}@example.com")).await;
    let (_, created) = call_json(
        &state,
        "POST",
        "/api/artifacts",
        Some(&owner),
        Some(json!({ "kind": "doc", "title": "Plan", "body": "v1", "visibility": "org" })),
    )
    .await;
    let id = str_at(&created, &["artifact", "id"]).to_string();
    let path = format!("/api/artifacts/{id}");
    // Readable by the org, editable by its owner only.
    let (s, shared) = call_json(
        &state,
        "PUT",
        &path,
        Some(&owner),
        Some(json!({ "visibility": "org", "editPolicy": "owner" })),
    )
    .await;
    assert_eq!(s, 200, "{shared}");

    let (s, mine) = call_json(&state, "GET", &path, Some(&owner), None).await;
    assert_eq!(s, 200);
    assert_eq!(mine["artifact"]["canEdit"], json!(true));
    let (s, theirs) = call_json(&state, "GET", &path, Some(&other), None).await;
    assert_eq!(s, 200, "{theirs}");
    assert_eq!(theirs["artifact"]["canEdit"], json!(false));

    agent_writes(&state.pg, Table::Artifacts, &id, "agent text").await;
    let t0 = str_at(&created, &["artifact", "updatedAt"]).to_string();
    let (s, refused) = call_json(
        &state,
        "PUT",
        &path,
        Some(&other),
        Some(json!({ "body": "theirs", "baseUpdatedAt": t0 })),
    )
    .await;
    assert_eq!(s, 403, "a non-editor must not be handed the row: {refused}");
    assert!(refused.get("artifact").is_none());
    assert_eq!(
        stored_body(&state.pg, Table::Artifacts, &id).await.0,
        "agent text"
    );

    sweep(&state.pg, &who, &[&id], &[], &[]).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL) and redis (REDIS_URL)"]
async fn kb_doc_put_refuses_a_stale_base_and_a_non_editor_gets_403() {
    let state = live_app_state().await;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    // Per test: the tests run in parallel, and a sweep must not take another
    // test's people with it.
    let who = format!("{PREFIX}-{tag}");
    let owner = minted_admin(&state, &who, &format!("owner-{tag}@example.com")).await;
    let (_other, other) =
        minted_member_with_email(&state, &who, &format!("other-{tag}@example.com")).await;
    let (s, space) = call_json(
        &state,
        "POST",
        "/api/kb/spaces",
        Some(&owner),
        Some(json!({ "name": format!("Save base {tag}"), "visibility": "org" })),
    )
    .await;
    assert!(s.is_success(), "space: {s} {space}");
    let space_id = str_at(&space, &["space", "id"]).to_string();
    let (s, created) = call_json(
        &state,
        "POST",
        &format!("/api/kb/spaces/{space_id}/docs"),
        Some(&owner),
        Some(json!({ "title": "Checklist", "body": "1. Tag the RC" })),
    )
    .await;
    assert!(s.is_success(), "doc: {s} {created}");
    let id = str_at(&created, &["doc", "id"]).to_string();
    let path = format!("/api/kb/docs/{id}");
    let (_, read) = call_json(&state, "GET", &path, Some(&owner), None).await;
    assert_eq!(read["doc"]["canEdit"], json!(true));
    let t0 = str_at(&read, &["doc", "updatedAt"]).to_string();

    agent_writes(
        &state.pg,
        Table::KbDocs,
        &id,
        "1. Tag the RC\n2. Agent: watch rc-deploy",
    )
    .await;

    let (s, refused) = call_json(
        &state,
        "PUT",
        &path,
        Some(&owner),
        Some(
            json!({ "title": "Checklist", "body": "1. Tag the RC, my words", "baseUpdatedAt": t0 }),
        ),
    )
    .await;
    assert_eq!(s, 409, "{refused}");
    assert_eq!(refused["conflict"], json!(true));
    assert_eq!(
        str_at(&refused, &["doc", "body"]),
        "1. Tag the RC\n2. Agent: watch rc-deploy"
    );
    assert_eq!(
        stored_body(&state.pg, Table::KbDocs, &id).await.0,
        "1. Tag the RC\n2. Agent: watch rc-deploy"
    );

    // On the fresh version the same save lands; with no base, it replaces.
    let fresh = str_at(&refused, &["doc", "updatedAt"]).to_string();
    let (s, landed) = call_json(
        &state,
        "PUT",
        &path,
        Some(&owner),
        Some(json!({ "body": "1. Tag the RC\n2. Agent: watch rc-deploy\n3. Mine", "baseUpdatedAt": fresh })),
    )
    .await;
    assert_eq!(s, 200, "{landed}");

    // A reader who may not edit: canEdit false on read, 403 before any 409.
    let (s, shared) = call_json(
        &state,
        "PUT",
        &path,
        Some(&owner),
        Some(json!({ "editPolicy": "owner" })),
    )
    .await;
    assert_eq!(s, 200, "{shared}");
    let (s, theirs) = call_json(&state, "GET", &path, Some(&other), None).await;
    assert_eq!(s, 200, "{theirs}");
    assert_eq!(theirs["doc"]["canEdit"], json!(false));
    let (s, denied) = call_json(
        &state,
        "PUT",
        &path,
        Some(&other),
        Some(json!({ "body": "theirs", "baseUpdatedAt": t0 })),
    )
    .await;
    assert_eq!(s, 403, "{denied}");
    assert!(denied.get("doc").is_none());

    sweep(&state.pg, &who, &[], &[&id], &[&space_id]).await;
}

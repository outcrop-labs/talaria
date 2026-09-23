mod support;
// Live-DB proof of the plan and research row actions (cargo test -- --ignored).
// DELETE /api/conversations/{id} archives an owned plan and ?hard=1 removes it;
// PATCH {archived:false} is the way back, owner-only. PATCH /api/research/{id}
// writes the title, and the Titler's conditional update does not clobber it.
// Each test drives the REAL router with a minted session, because the gates
// (owner vs collaborator vs stranger, the exact ?hard=1 / ?archived=1
// spellings) live on the route, not in a helper beside it.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test plan_row_live -- --ignored

use axum::body::Body;
use axum::http::Request;
use sqlx::PgPool;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use tower::ServiceExt;

async fn app_state() -> AppState {
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL (source ui/.env)"),
        std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)"),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

/// Each test owns its tag. The binary's tests run concurrently, and a shared
/// email prefix would let one test's cleanup delete another's rows.
async fn reset(pg: &PgPool, tag: &str) {
    sqlx::query("delete from research_runs where requested_by like $1")
        .bind(format!("plan-row-{tag}-%@test.invalid"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where email like $1")
        .bind(format!("plan-row-{tag}-%@test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

async fn user_row(pg: &PgPool, tag: &str, who: &str, role: &str) -> SessionUser {
    let email = format!("plan-row-{tag}-{who}@test.invalid");
    let sub = format!("plan-row-live:{email}");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, $4) returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .bind(who)
    .bind(role)
    .fetch_one(pg)
    .await
    .unwrap();
    SessionUser {
        id,
        sub,
        email: Some(email),
        name: Some(who.to_string()),
        picture: None,
        role: role.to_string(),
        provider: "google".into(),
    }
}

async fn plan(pg: &PgPool, owner: &str, title: &str) -> String {
    sqlx::query_scalar(
        "insert into conversations (user_id, agent_model, title, kind) \
         values ($1::uuid, 'live-test-model', $2, 'plan') returning id::text",
    )
    .bind(owner)
    .bind(title)
    .fetch_one(pg)
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

async fn sid(state: &AppState, user: &SessionUser) -> String {
    create_session(state, user).await.unwrap()
}

async fn call(
    state: &AppState,
    session: &str,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> (u16, String) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", format!("talaria_session={session}"));
    let req = match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let res = routes::router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn archived_flag(pg: &PgPool, id: &str) -> Option<bool> {
    sqlx::query_scalar("select archived from conversations where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
        .unwrap()
}

/// DELETE without ?hard=1 hides the plan from the live list and leaves the
/// row. A collaborator cannot do it. The owner can bring it back.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn an_owner_archives_a_plan_and_a_collaborator_cannot() {
    let state = app_state().await;
    let tag = "archive";
    reset(&state.pg, tag).await;
    let owner = user_row(&state.pg, tag, "owner", "member").await;
    let mate = user_row(&state.pg, tag, "mate", "member").await;
    let stranger = user_row(&state.pg, tag, "stranger", "member").await;
    let id = plan(&state.pg, &owner.id, "Quarter plan").await;
    share(&state.pg, &id, &mate.id).await;
    let owner_sid = sid(&state, &owner).await;
    let mate_sid = sid(&state, &mate).await;
    let stranger_sid = sid(&state, &stranger).await;

    let (status, _) = call(
        &state,
        &mate_sid,
        "DELETE",
        &format!("/api/conversations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 403, "a collaborator must not hide the owner's plan");
    assert_eq!(archived_flag(&state.pg, &id).await, Some(false));

    let (status, _) = call(
        &state,
        &stranger_sid,
        "DELETE",
        &format!("/api/conversations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 404, "a stranger learns nothing about the id");

    let (status, body) = call(
        &state,
        &owner_sid,
        "DELETE",
        &format!("/api/conversations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(archived_flag(&state.pg, &id).await, Some(true));

    let (_, live) = call(
        &state,
        &owner_sid,
        "GET",
        "/api/conversations?kind=plan",
        None,
    )
    .await;
    assert!(!live.contains(&id), "archived plans leave the live list");
    let (_, retired) = call(
        &state,
        &owner_sid,
        "GET",
        "/api/conversations?kind=plan&archived=1",
        None,
    )
    .await;
    assert!(retired.contains(&id), "the retired list is where it went");

    // The record is still openable — archive hides the list row, it does not
    // revoke the link.
    let (status, _) = call(
        &state,
        &owner_sid,
        "GET",
        &format!("/api/conversations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);

    let (status, _) = call(
        &state,
        &mate_sid,
        "PATCH",
        &format!("/api/conversations/{id}"),
        Some(r#"{"archived":false}"#),
    )
    .await;
    assert_eq!(status, 403, "restore is the owner's");

    let (status, _) = call(
        &state,
        &owner_sid,
        "PATCH",
        &format!("/api/conversations/{id}"),
        Some(r#"{"archived":false}"#),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(archived_flag(&state.pg, &id).await, Some(false));
    let (_, live) = call(
        &state,
        &owner_sid,
        "GET",
        "/api/conversations?kind=plan",
        None,
    )
    .await;
    assert!(live.contains(&id), "restore puts it back on the live list");

    reset(&state.pg, tag).await;
}

/// ?hard=1 removes the row. A title rename still works for a collaborator,
/// and PATCH refuses to be a second archive door.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn hard_delete_removes_the_plan_and_patch_does_not_archive() {
    let state = app_state().await;
    let tag = "hard";
    reset(&state.pg, tag).await;
    let owner = user_row(&state.pg, tag, "owner", "member").await;
    let mate = user_row(&state.pg, tag, "mate", "member").await;
    let id = plan(&state.pg, &owner.id, "Throwaway").await;
    share(&state.pg, &id, &mate.id).await;
    let owner_sid = sid(&state, &owner).await;
    let mate_sid = sid(&state, &mate).await;

    let (status, _) = call(
        &state,
        &mate_sid,
        "PATCH",
        &format!("/api/conversations/{id}"),
        Some(r#"{"title":"Mate's name"}"#),
    )
    .await;
    assert_eq!(status, 200, "a collaborator may still rename");

    let (status, body) = call(
        &state,
        &owner_sid,
        "PATCH",
        &format!("/api/conversations/{id}"),
        Some(r#"{"archived":true}"#),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("archive with DELETE"));
    assert_eq!(archived_flag(&state.pg, &id).await, Some(false));

    let (status, body) = call(
        &state,
        &owner_sid,
        "PATCH",
        &format!("/api/conversations/{id}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 400, "{body}");

    let (status, _) = call(
        &state,
        &owner_sid,
        "DELETE",
        &format!("/api/conversations/{id}?hard=1"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(archived_flag(&state.pg, &id).await, None, "the row is gone");
    let (status, _) = call(
        &state,
        &owner_sid,
        "GET",
        &format!("/api/conversations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 404);

    reset(&state.pg, tag).await;
}

async fn research_run(pg: &PgPool, owner: &str, tag: &str, question: &str) -> String {
    let requested = format!("plan-row-{tag}-owner@test.invalid");
    sqlx::query_scalar(
        "insert into research_runs (owner_user_id, requested_by, agent_model, mode, question) \
         values ($1::uuid, $2, 'live-test-model', 'brief', $3) returning id::text",
    )
    .bind(owner)
    .bind(&requested)
    .bind(question)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// A renamed title sticks. The Titler's write is conditional on a null title,
/// so a name that landed first is not replaced by the question's generated one.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn a_renamed_research_title_is_not_clobbered() {
    let state = app_state().await;
    let tag = "rename";
    reset(&state.pg, tag).await;
    let owner = user_row(&state.pg, tag, "owner", "member").await;
    let other = user_row(&state.pg, tag, "other", "member").await;
    let admin = user_row(&state.pg, tag, "admin", "admin").await;
    let id = research_run(&state.pg, &owner.id, tag, "What changed in the tariff?").await;
    let owner_sid = sid(&state, &owner).await;
    let other_sid = sid(&state, &other).await;
    let admin_sid = sid(&state, &admin).await;

    let (status, _) = call(
        &state,
        &other_sid,
        "PATCH",
        &format!("/api/research/{id}"),
        Some(r#"{"title":"nope"}"#),
    )
    .await;
    assert_eq!(status, 403);

    let (status, body) = call(
        &state,
        &owner_sid,
        "PATCH",
        &format!("/api/research/{id}"),
        Some(r#"{"title":"Tariff note"}"#),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // The fire-and-forget Titler write, exactly as start_research spells it.
    sqlx::query("update research_runs set title = $1 where id = $2::uuid and title is null")
        .bind("generated")
        .bind(&id)
        .execute(&state.pg)
        .await
        .unwrap();
    let title: Option<String> =
        sqlx::query_scalar("select title from research_runs where id = $1::uuid")
            .bind(&id)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(title.as_deref(), Some("Tariff note"));

    let (status, _) = call(
        &state,
        &admin_sid,
        "PATCH",
        &format!("/api/research/{id}"),
        Some(r#"{"title":"Admin note"}"#),
    )
    .await;
    assert_eq!(status, 200, "admin may rename, same as delete");

    let (status, _) = call(
        &state,
        &owner_sid,
        "PATCH",
        &format!("/api/research/{id}"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, 400);

    reset(&state.pg, tag).await;
}

// Live-DB + live-Redis proof that a well-formed path id that names nothing is
// a 404 on the writes that used to answer otherwise, and that the database's
// own text never reaches the caller:
//
// - PUT /api/fleet/agents/{id}/secrets answered 400 with the foreign key's
//   refusal ("secret write failed: error returned from database: …");
// - PUT /api/workflows/{id}, PUT and DELETE /api/fleet/endpoints/{id}, and
//   DELETE /api/workbench/coding/login/{id} answered 200 for a write that
//   touched nothing.
//
// A PUT to nothing is a 404, and so is a DELETE of something the route
// addresses by its own id (the house's DELETE answer: blueprints, tasks,
// templates, MCP servers, git hosts). The second test proves the rows that do
// exist still take the write, in every spelling Postgres' uuid input takes.
//
// Round 6 (AP1): the last five DELETEs that answered 200 for an id naming
// nothing (workflows, keys, rag collections, artifact folders, an agent's
// secrets) answer 404 too, write no audit row for it, and the secrets GET of
// an agent that does not exist is the same 404 instead of an empty list. The
// third test deletes a real row of each, in a spelling other than the stored
// one, and the second delete of it is the 404.
//
// The agent is unmanaged, so nothing here renders or touches a container.
// House rule: #[ignore]d; CI's api-integration job runs it.
//
//   source ui/.env && cargo test -p talaria-api --test it absent_path_ids:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::gateway::registry::invalidate_endpoints_cache;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state};

const PREFIX: &str = "absent-path-ids";

/// Each test sweeps only the rows its own `tag` made: the tests in a file run
/// concurrently, and a sweep of the whole prefix deletes a sibling's rows
/// mid-flight.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from task_workflows where name like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from llm_endpoints where name like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    invalidate_endpoints_cache();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// An admin with a session minted the way the auth routes mint one: it
/// passes every agent-manager and permission gate, so each request reaches
/// the id.
async fn minted_admin(state: &AppState, tag: &str) -> String {
    let sub = format!("{PREFIX}:{tag}:{}", uuid::Uuid::new_v4());
    let (id,): (String,) =
        sqlx::query_as("insert into users (sub, role) values ($1, 'admin') returning id::text")
            .bind(&sub)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let user = SessionUser {
        id,
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
    format!("talaria_session={sid}")
}

/// What no answer may carry: the database speaking.
fn names_the_database(answer: &Value) -> bool {
    let text = answer.to_string().to_ascii_lowercase();
    [
        "database",
        "foreign key",
        "constraint",
        "violates",
        "syntax",
        "sqlx",
    ]
    .iter()
    .any(|w| text.contains(w))
}

/// Every spelling Postgres' uuid input takes for one id. `{…}` is
/// percent-encoded, as a URL carries it; the router decodes it.
fn spellings(id: &str) -> [String; 4] {
    [
        id.to_string(),
        id.to_ascii_uppercase(),
        format!("%7B{id}%7D"),
        id.replace('-', ""),
    ]
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_well_formed_id_that_names_nothing_is_a_404_and_never_the_database() {
    let state = live_app_state().await;
    cleanup(&state.pg, "absent").await;
    let cookie = minted_admin(&state, "absent").await;
    let absent = uuid::Uuid::new_v4().to_string();

    let mut wrong = Vec::new();
    for id in spellings(&absent) {
        let cases: Vec<(&str, String, Option<Value>, Option<&str>)> = vec![
            (
                "PUT",
                format!("/api/fleet/agents/{id}/secrets"),
                Some(json!({ "name": "ABSENT_TOKEN", "value": "x" })),
                Some(cookie.as_str()),
            ),
            (
                "PUT",
                format!("/api/workflows/{id}"),
                Some(json!({ "name": "renamed" })),
                Some(cookie.as_str()),
            ),
            // An empty patch writes nothing either way; to nothing, it is
            // still a 404.
            (
                "PUT",
                format!("/api/workflows/{id}"),
                Some(json!({})),
                Some(cookie.as_str()),
            ),
            (
                "PUT",
                format!("/api/fleet/endpoints/{id}"),
                Some(json!({ "class": "local" })),
                Some(cookie.as_str()),
            ),
            (
                "PUT",
                format!("/api/fleet/endpoints/{id}"),
                Some(json!({})),
                Some(cookie.as_str()),
            ),
            (
                "DELETE",
                format!("/api/fleet/endpoints/{id}"),
                None,
                Some(cookie.as_str()),
            ),
            (
                "DELETE",
                format!("/api/workbench/coding/login/{id}"),
                None,
                Some(cookie.as_str()),
            ),
            // The poll and the answer beside it give an unknown session the
            // same 404 with no session at all; so does the abandon.
            (
                "DELETE",
                format!("/api/workbench/coding/login/{id}"),
                None,
                None,
            ),
        ];
        let mut cases = cases;
        cases.extend([
            (
                "DELETE",
                format!("/api/workflows/{id}"),
                None,
                Some(cookie.as_str()),
            ),
            (
                "DELETE",
                format!("/api/keys/{id}"),
                None,
                Some(cookie.as_str()),
            ),
            (
                "DELETE",
                format!("/api/rag/collections/{id}"),
                None,
                Some(cookie.as_str()),
            ),
            (
                "DELETE",
                format!("/api/artifact-folders/{id}"),
                None,
                Some(cookie.as_str()),
            ),
            (
                "DELETE",
                format!("/api/fleet/agents/{id}/secrets"),
                Some(json!({ "name": "ABSENT_TOKEN" })),
                Some(cookie.as_str()),
            ),
            (
                "GET",
                format!("/api/fleet/agents/{id}/secrets"),
                None,
                Some(cookie.as_str()),
            ),
        ]);
        for (method, uri, body, cookie) in cases {
            let (status, answer) = call_json(&state, method, &uri, cookie, body).await;
            if status != StatusCode::NOT_FOUND || names_the_database(&answer) {
                wrong.push(format!("{method} {uri} → {status} {answer}"));
            }
        }
    }
    // Nothing was deleted, so nothing is audited: the audits are written off
    // the request's clock, so give them a moment to land if they were going to.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let audited: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where lower(replace(replace(replace(target_id, '{', ''), '}', ''), '-', '')) = $1",
    )
    .bind(absent.replace('-', ""))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(audited, 0, "a delete of nothing wrote an audit row");
    // A malformed endpoint id with a field to write was the cast's 500.
    for bad in [
        "not-a-uuid",
        "urn:uuid:6113b5bc-6935-46e1-a67c-60bd58daf4f0",
    ] {
        let uri = format!("/api/fleet/endpoints/{bad}");
        let (status, answer) = call_json(
            &state,
            "PUT",
            &uri,
            Some(cookie.as_str()),
            Some(json!({ "class": "local" })),
        )
        .await;
        if status != StatusCode::NOT_FOUND || names_the_database(&answer) {
            wrong.push(format!("PUT {uri} → {status} {answer}"));
        }
    }
    let written: i64 =
        sqlx::query_scalar("select count(*) from agent_secrets where name = 'ABSENT_TOKEN'")
            .fetch_one(&state.pg)
            .await
            .unwrap();
    cleanup(&state.pg, "absent").await;
    assert!(
        wrong.is_empty(),
        "these answered something other than a plain 404:\n{}",
        wrong.join("\n")
    );
    assert_eq!(written, 0);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_rows_that_exist_still_take_the_write_in_every_spelling() {
    let state = live_app_state().await;
    cleanup(&state.pg, "present").await;
    let cookie = minted_admin(&state, "present").await;

    // An unmanaged agent takes its secret.
    let slug = format!("{PREFIX}-present-{}", uuid::Uuid::new_v4());
    let (agent,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, managed, enabled) \
         values ($1, $1, $1, 'Absent Path Ids', false, false) returning id::text",
    )
    .bind(&slug)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (status, answer) = call_json(
        &state,
        "PUT",
        &format!("/api/fleet/agents/{agent}/secrets"),
        Some(cookie.as_str()),
        Some(json!({ "name": "PRESENT_TOKEN", "value": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let held: i64 = sqlx::query_scalar(
        "select count(*) from agent_secrets where agent_id = $1::uuid and name = 'PRESENT_TOKEN'",
    )
    .bind(&agent)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert_eq!(held, 1);
    // A refused name is still the route's own 400.
    let (status, answer) = call_json(
        &state,
        "PUT",
        &format!("/api/fleet/agents/{agent}/secrets"),
        Some(cookie.as_str()),
        Some(json!({ "name": "lower_case", "value": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");

    // A workflow takes a patch, and an empty one, in every spelling.
    let (workflow,): (String,) =
        sqlx::query_as("insert into task_workflows (name) values ($1) returning id::text")
            .bind(format!("{PREFIX}-present-wf"))
            .fetch_one(&state.pg)
            .await
            .unwrap();
    for (i, id) in spellings(&workflow).into_iter().enumerate() {
        let name = format!("{PREFIX}-present-wf-{i}");
        for body in [json!({ "name": name }), json!({})] {
            let (status, answer) = call_json(
                &state,
                "PUT",
                &format!("/api/workflows/{id}"),
                Some(cookie.as_str()),
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "PUT /api/workflows/{id} → {answer}");
        }
        let (now,): (String,) =
            sqlx::query_as("select name from task_workflows where id = $1::uuid")
                .bind(&workflow)
                .fetch_one(&state.pg)
                .await
                .unwrap();
        assert_eq!(now, name);
    }

    // An endpoint takes its edit in every spelling, then its delete.
    let (endpoint,): (String,) = sqlx::query_as(
        "insert into llm_endpoints (name, provider, class) values ($1, 'openai', 'cloud') \
         returning id::text",
    )
    .bind(format!("{PREFIX}-present-ep"))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    invalidate_endpoints_cache();
    for (i, id) in spellings(&endpoint).into_iter().enumerate() {
        let class = if i % 2 == 0 { "local" } else { "cloud" };
        let (status, answer) = call_json(
            &state,
            "PUT",
            &format!("/api/fleet/endpoints/{id}"),
            Some(cookie.as_str()),
            Some(json!({ "class": class })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "PUT /api/fleet/endpoints/{id} → {answer}"
        );
        let (now,): (String,) =
            sqlx::query_as("select class from llm_endpoints where id = $1::uuid")
                .bind(&endpoint)
                .fetch_one(&state.pg)
                .await
                .unwrap();
        assert_eq!(now, class, "{id}");
    }
    let upper = endpoint.to_ascii_uppercase();
    let (status, answer) = call_json(
        &state,
        "DELETE",
        &format!("/api/fleet/endpoints/{upper}"),
        Some(cookie.as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let left: i64 = sqlx::query_scalar("select count(*) from llm_endpoints where id = $1::uuid")
        .bind(&endpoint)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    assert_eq!(left, 0);
    // Gone now: the second delete removed nothing.
    let (status, _) = call_json(
        &state,
        "DELETE",
        &format!("/api/fleet/endpoints/{endpoint}"),
        Some(cookie.as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The audits are written off the request's clock; give them a moment
    // before the rows they name are swept with the rest.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    sqlx::query("delete from audit_log where target_id = any($1)")
        .bind(vec![agent.clone(), workflow.clone(), endpoint.clone()])
        .execute(&state.pg)
        .await
        .unwrap();
    cleanup(&state.pg, "present").await;
}

/// The five DELETEs that answer 404 for nothing still delete what is there,
/// in another spelling of its id, and the second delete of it is the 404.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_five_deletes_still_delete_what_is_there() {
    let state = live_app_state().await;
    cleanup(&state.pg, "delete").await;
    let cookie = minted_admin(&state, "delete").await;
    let (admin,): (String,) = sqlx::query_as(
        "select id::text from users where sub like $1 order by created_at desc limit 1",
    )
    .bind(format!("{PREFIX}:delete:%"))
    .fetch_one(&state.pg)
    .await
    .unwrap();

    let (workflow,): (String,) =
        sqlx::query_as("insert into task_workflows (name) values ($1) returning id::text")
            .bind(format!("{PREFIX}-delete-wf"))
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let (key,): (String,) = sqlx::query_as(
        "insert into llm_api_keys (user_id, name, key_hash, prefix) \
         values ($1::uuid, $2, $3, 'tlk_test') returning id::text",
    )
    .bind(&admin)
    .bind(format!("{PREFIX}-delete-key"))
    .bind(format!("{PREFIX}-delete-{}", uuid::Uuid::new_v4()))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (collection,): (String,) = sqlx::query_as(
        "insert into rag_collections (name, kind, qdrant_name) values ($1, 'custom', $1) \
         returning id::text",
    )
    .bind(format!("{PREFIX}-delete-rag-{}", uuid::Uuid::new_v4()))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (folder,): (String,) = sqlx::query_as(
        "insert into artifact_folders (name, created_by) values ($1, $2) returning id::text",
    )
    .bind(format!("{PREFIX}-delete-folder"))
    .bind(&admin)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let slug = format!("{PREFIX}-delete-{}", uuid::Uuid::new_v4());
    let (agent,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, managed, enabled) \
         values ($1, $1, $1, 'Absent Path Ids', false, false) returning id::text",
    )
    .bind(&slug)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (status, answer) = call_json(
        &state,
        "PUT",
        &format!("/api/fleet/agents/{agent}/secrets"),
        Some(cookie.as_str()),
        Some(json!({ "name": "DELETE_TOKEN", "value": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    // The secrets list reads the agent in any spelling.
    let (status, answer) = call_json(
        &state,
        "GET",
        &format!("/api/fleet/agents/{}/secrets", agent.to_ascii_uppercase()),
        Some(cookie.as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["secrets"][0]["name"], "DELETE_TOKEN", "{answer}");

    let upper = |id: &str| id.to_ascii_uppercase();
    let braced = |id: &str| format!("%7B{id}%7D");
    let cases: Vec<(String, Option<Value>, &str)> = vec![
        (
            format!("/api/workflows/{}", upper(&workflow)),
            None,
            "task_workflows",
        ),
        (format!("/api/keys/{}", braced(&key)), None, "llm_api_keys"),
        (
            format!("/api/rag/collections/{}", upper(&collection)),
            None,
            "rag_collections",
        ),
        (
            format!("/api/artifact-folders/{}", folder.replace('-', "")),
            None,
            "artifact_folders",
        ),
        (
            format!("/api/fleet/agents/{}/secrets", upper(&agent)),
            Some(json!({ "name": "DELETE_TOKEN" })),
            "agent_secrets",
        ),
    ];
    for (uri, body, _) in &cases {
        let (status, answer) =
            call_json(&state, "DELETE", uri, Some(cookie.as_str()), body.clone()).await;
        assert_eq!(status, StatusCode::OK, "DELETE {uri} → {answer}");
    }
    // Each row is gone (a revoked key stays, stamped).
    let gone = |sql: &'static str, id: String| {
        let pg = state.pg.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(id)
                .fetch_one(&pg)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        gone(
            "select count(*) from task_workflows where id = $1::uuid",
            workflow.clone()
        )
        .await,
        0
    );
    assert_eq!(
        gone(
            "select count(*) from llm_api_keys where id = $1::uuid and revoked_at is null",
            key.clone()
        )
        .await,
        0
    );
    assert_eq!(
        gone(
            "select count(*) from rag_collections where id = $1::uuid",
            collection.clone()
        )
        .await,
        0
    );
    assert_eq!(
        gone(
            "select count(*) from artifact_folders where id = $1::uuid",
            folder.clone()
        )
        .await,
        0
    );
    assert_eq!(
        gone(
            "select count(*) from agent_secrets where agent_id = $1::uuid",
            agent.clone()
        )
        .await,
        0
    );
    // The secret's audit names the agent's own id, not the path's spelling.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let audit: Option<String> = sqlx::query_scalar(
        "select target_id from audit_log where action = 'agent.secret_delete' \
         and lower(target_id) = $1 order by created_at desc limit 1",
    )
    .bind(&agent)
    .fetch_optional(&state.pg)
    .await
    .unwrap();
    assert_eq!(audit.as_deref(), Some(agent.as_str()));

    // Gone now: the second delete of each is the 404, except the key, which
    // is still the owner's (revoked), and the secret, whose agent is still
    // there: a name with nothing under it stays the idempotent ok.
    for (uri, body, table) in &cases {
        let (status, answer) =
            call_json(&state, "DELETE", uri, Some(cookie.as_str()), body.clone()).await;
        let want = match *table {
            "llm_api_keys" | "agent_secrets" => StatusCode::OK,
            _ => StatusCode::NOT_FOUND,
        };
        assert_eq!(status, want, "DELETE {uri} again → {answer}");
    }
    // Someone else's key is the same 404 as nobody's.
    let (other,): (String,) =
        sqlx::query_as("insert into users (sub, role) values ($1, 'member') returning id::text")
            .bind(format!("{PREFIX}:delete:{}", uuid::Uuid::new_v4()))
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let (theirs,): (String,) = sqlx::query_as(
        "insert into llm_api_keys (user_id, name, key_hash, prefix) \
         values ($1::uuid, $2, $3, 'tlk_test') returning id::text",
    )
    .bind(&other)
    .bind(format!("{PREFIX}-delete-theirs"))
    .bind(format!("{PREFIX}-delete-{}", uuid::Uuid::new_v4()))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (status, _) = call_json(
        &state,
        "DELETE",
        &format!("/api/keys/{theirs}"),
        Some(cookie.as_str()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        gone(
            "select count(*) from llm_api_keys where id = $1::uuid and revoked_at is null",
            theirs.clone()
        )
        .await,
        1,
        "someone else's key stays live"
    );

    sqlx::query("delete from audit_log where lower(target_id) = any($1)")
        .bind(vec![
            workflow.clone(),
            key.clone(),
            collection.clone(),
            folder.clone(),
            agent.clone(),
        ])
        .execute(&state.pg)
        .await
        .unwrap();
    sqlx::query("delete from artifact_folders where name like $1")
        .bind(format!("{PREFIX}-delete-%"))
        .execute(&state.pg)
        .await
        .unwrap();
    sqlx::query("delete from rag_collections where name like $1")
        .bind(format!("{PREFIX}-delete-%"))
        .execute(&state.pg)
        .await
        .unwrap();
    cleanup(&state.pg, "delete").await;
}

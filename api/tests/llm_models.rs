// Router-level tests for /api/llm/v1/models — the exact stack the process
// serves (routes::router), driven without a socket via tower's oneshot.
//
// Two kinds live here:
//   • CI-safe (no services — house rule): the missing/bogus-header 401 paths.
//     These exit before any query: authenticate_key's tlk_ gate runs first,
//     exactly as in TS, so no database is consulted.
//   • #[ignore]d live-DB tests, run locally with `cargo test -- --ignored`
//     against a real dev database (DATABASE_URL). Never in CI.

use axum::body::Body;
use axum::http::Request;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

fn test_config() -> Config {
    // The pool is lazy — nothing connects unless a handler queries.
    Config::from_parts(
        "postgres://t:t@127.0.0.1:1/unused".into(),
        "redis://127.0.0.1:1".into(),
        "test-root".into(),
        String::new(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles")
}

async fn get(path: &str, auth: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(path);
    if let Some(a) = auth {
        builder = builder.header("authorization", a);
    }
    let state = AppState::new(
        talaria_api::db::pool(&test_config()),
        std::sync::Arc::new(test_config()),
    );
    routes::router(state)
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn missing_bearer_is_the_exact_ts_401() {
    let res = get("/api/llm/v1/models", None).await;
    assert_eq!(res.status(), 401);
    assert_eq!(
        res.headers().get("content-type").unwrap(),
        "application/json"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    // The OpenAI envelope, byte-pinned: no type/code fields, message only.
    assert_eq!(&body[..], br#"{"error":{"message":"invalid API key"}}"#);
}

#[tokio::test]
async fn non_tlk_bearer_never_touches_the_database() {
    // A gateway key always starts tlk_; anything else is rejected before the
    // query — provable here because this test has NO database to touch.
    let res = get("/api/llm/v1/models", Some("Bearer sk-not-a-talaria-key")).await;
    assert_eq!(res.status(), 401);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], br#"{"error":{"message":"invalid API key"}}"#);
}

// ── live-DB tests (cargo test -- --ignored) ───────────────────────────────────

fn live_config() -> Option<Config> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    Config::from_parts(
        url,
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6399".into()),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        std::env::var("AUTH_SECRET").unwrap_or_default(),
        String::new(),
    )
    .ok()
}

/// A tlk_-shaped secret nobody minted: exercises the real query and the
/// miss → 401 path against the same rows TS would read.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn unknown_tlk_key_401s_via_a_real_query() {
    let Some(cfg) = live_config() else {
        panic!("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    };
    let state = AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg));
    let res = routes::router(state)
        .oneshot(
            Request::builder()
                .uri("/api/llm/v1/models")
                .header(
                    "authorization",
                    "Bearer tlk_deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], br#"{"error":{"message":"invalid API key"}}"#);
}

/// With TALARIA_LLM_TEST_KEY minted in the dev DB (a real tlk_ secret), the
/// full path: auth → catalog → the exact list body.
#[tokio::test]
#[ignore = "needs a live dev database + TALARIA_LLM_TEST_KEY holding a minted tlk_ secret"]
async fn minted_key_lists_the_catalog() {
    let Some(cfg) = live_config() else {
        panic!("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    };
    let secret =
        std::env::var("TALARIA_LLM_TEST_KEY").expect("mint a key and export TALARIA_LLM_TEST_KEY");
    assert!(
        secret.starts_with("tlk_"),
        "a gateway key always starts tlk_"
    );
    let state = AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg));
    let res = routes::router(state)
        .oneshot(
            Request::builder()
                .uri("/api/llm/v1/models")
                .header("authorization", format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["object"], "list");
    assert!(
        v["data"].as_array().is_some_and(|a| !a.is_empty()),
        "dev database has endpoints configured"
    );
    for m in v["data"].as_array().unwrap() {
        assert_eq!(m["object"], "model");
        assert!(m["owned_by"].as_str().unwrap().starts_with("talaria:"));
    }
}

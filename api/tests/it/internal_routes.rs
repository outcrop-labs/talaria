// Router-level tests for /api/internal/* — the app host's own calls to the
// api, which answer only the root-derived service credential
// (`require_internal`). Driven through the exact stack the process serves
// (routes::router) with tower's oneshot.
//
//   • CI-safe (no services): every refusal. The guard answers from the
//     header before any query, so the lazy pool never connects.
//   • #[ignore]d live: the credential is admitted and the sync answers. CI's
//     api-integration job runs it against the migrated scratch database.
//
// Its own module, not beside app_mcp_sync_live: CI runs one module at a time,
// and a sync driven from here must not run inside that test's sequence.

use axum::body::Body;
use axum::http::Request;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

const SYNC: &str = "/api/internal/apps/mcp-sync";

fn test_state() -> AppState {
    // The pool is lazy — nothing connects unless a handler queries.
    let cfg = || {
        Config::from_parts(
            "postgres://t:t@127.0.0.1:1/unused".into(),
            "redis://127.0.0.1:1".into(),
            "test-root".into(),
            String::new(),
            String::new(),
            String::new(),
        )
        .expect("test config assembles")
    };
    AppState::new(talaria_api::db::pool(&cfg()), std::sync::Arc::new(cfg()))
}

async fn call(state: AppState, method: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut req = Request::builder().method(method).uri(SYNC);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = routes::router(state)
        .oneshot(req.body(Body::from("{}")).unwrap())
        .await
        .unwrap();
    let status = res.status().as_u16();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn the_sync_refuses_anyone_without_the_internal_token() {
    let state = test_state();
    let token = state.cfg.secret_root.internal_token();
    let header = talaria_api::config::INTERNAL_HEADER;
    for headers in [
        vec![],
        vec![("cookie", "talaria_session=an-admins-session")],
        vec![("authorization", "Bearer tak_an-agents-key")],
        vec![(header, "not-the-token")],
        vec![(header, &token[..63])],
    ] {
        let (status, body) = call(state.clone(), "POST", &headers).await;
        assert_eq!(status, 403, "{headers:?} → {body}");
        assert!(body.contains("only the app host may call it"), "{body}");
    }
    let (status, _) = call(state, "GET", &[(header, &token)]).await;
    assert_eq!(status, 405);
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_sync_answers_the_host_with_the_servers_that_serve() {
    let state = crate::support::app_state().await;
    let token = state.cfg.secret_root.internal_token();
    let (status, body) = call(
        state,
        "POST",
        &[(talaria_api::config::INTERNAL_HEADER, &token)],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], true);
    assert!(v["servers"].is_array(), "{body}");
}

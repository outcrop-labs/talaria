// GET /api/artifacts/{id} — the one-artifact read.
//
// A well-formed id that matches no row is an honest 404, answered before
// auth, never a 500. A transient read failure is a different shape (the
// retryable sentence on the GET Err arms); this pins the miss so the two
// cannot collapse. Needs a live database: the miss is a real query.

use axum::body::Body;
use axum::http::Request;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::state::AppState;
use tower::ServiceExt;

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

/// The id the incident used as the unknown-but-well-formed control. Not a
/// row; a cast error or a mapped Err here would be a 500, which is the
/// regression.
const UNKNOWN_ID: &str = "00000000-0000-4000-8000-000000000000";

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn unknown_well_formed_id_is_404_not_500() {
    let Some(cfg) = live_config() else {
        panic!("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    };
    let state = AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg));
    let res = routes::router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/artifacts/{UNKNOWN_ID}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], br#"{"error":"not found"}"#);
}

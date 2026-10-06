// Live-DB + live-Redis proof for the identity fields (comms polish U1): the
// profile photo (`users.avatar_upload_id`, claimed through PUT /api/me and
// served by GET /api/users/{id}/avatar), status, and presence. Each test
// drives the REAL router through a minted session — the claim checks, the
// session push, the effective-picture URL and the presence MGET are all
// decided by SQL and Redis that no unit test can run.
//
// The pure halves (the claim refusals, the effective-picture helper, the
// presence outage mapping, the patch bounds) are unit-tested beside their
// code in talaria-users and talaria-routes-integrations.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it profile_live:: -- --ignored

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use talaria_api::routes;
use talaria_api::state::AppState;
use talaria_api::users::{Identity, upsert_user};
use tower::ServiceExt;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "profile-live";

async fn member(state: &AppState) -> (String, String, String) {
    let email = format!("{}@profile-live.invalid", uuid::Uuid::new_v4());
    let (id, cookie) = minted_member_with_email(state, PREFIX, &email).await;
    (id, cookie, email)
}

/// Upload `bytes` as `filename` with the part's declared `mime` — the stored
/// mime is what the uploader's client said, which is exactly what the avatar
/// claim must check.
async fn upload(
    state: &AppState,
    cookie: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> String {
    let boundary = "profile-live";
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let res = routes::router(state.clone())
        .oneshot(
            Request::post("/api/uploads")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("cookie", cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the upload itself is a normal upload");
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&raw).unwrap();
    v["id"].as_str().expect("upload id").to_string()
}

async fn avatar_column(state: &AppState, user_id: &str) -> Option<String> {
    sqlx::query_scalar("select avatar_upload_id::text from users where id = $1::uuid")
        .bind(user_id)
        .fetch_one(&state.pg)
        .await
        .unwrap()
}

fn avatar_url(user_id: &str, upload_id: &str) -> String {
    format!("/api/users/{user_id}/avatar?v={}", &upload_id[..8])
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_owned_png_becomes_the_photo_everyone_sees() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
    let up = upload(&state, &cookie, "me.png", "image/png", &png).await;

    let (status, body) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": up })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["picture"], avatar_url(&me, &up));
    assert_eq!(
        avatar_column(&state, &me).await.as_deref(),
        Some(up.as_str())
    );

    // The session the SPA reads carries the same effective picture.
    let (_, session) = call_json(&state, "GET", "/api/auth/session", Some(&cookie), None).await;
    assert_eq!(session["user"]["picture"], avatar_url(&me, &up));

    // A different member gets the bytes, immutably cached.
    let (_, other_cookie, _) = member(&state).await;
    let res = routes::router(state.clone())
        .oneshot(
            Request::get(format!("/api/users/{me}/avatar"))
                .header("cookie", &other_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers().get("cache-control").unwrap(),
        "private, max-age=31536000, immutable"
    );
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(raw.as_ref(), png.as_slice());

    // Nobody signed in: no bytes.
    let (status, _) = call_json(
        &state,
        "GET",
        &format!("/api/users/{me}/avatar"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_user_without_a_photo_has_no_avatar() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (status, _) = call_json(
        &state,
        "GET",
        &format!("/api/users/{me}/avatar"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn someone_elses_upload_is_refused_and_changes_nothing() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (_, other_cookie, _) = member(&state).await;
    let theirs = upload(&state, &other_cookie, "them.png", "image/png", b"png").await;
    let (status, body) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": theirs, "statusEmoji": "📅" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["error"].is_string());
    assert_eq!(avatar_column(&state, &me).await, None);
    let emoji: Option<String> =
        sqlx::query_scalar("select status_emoji from users where id = $1::uuid")
            .bind(&me)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(emoji, None, "a refused photo writes nothing else either");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_pdf_or_an_oversized_image_is_a_400() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let pdf = upload(&state, &cookie, "doc.pdf", "application/pdf", b"%PDF-1.4").await;
    let (status, body) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": pdf })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"],
        "a profile photo must be a PNG, JPEG, WebP, or GIF image"
    );

    let big = upload(
        &state,
        &cookie,
        "big.png",
        "image/png",
        &vec![7u8; 6 * 1024 * 1024],
    )
    .await;
    let (status, body) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": big })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "a profile photo must be 5 MB or smaller");
    assert_eq!(avatar_column(&state, &me).await, None);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn clearing_the_photo_restores_the_google_picture() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    sqlx::query("update users set picture = 'https://google.test/me.png' where id = $1::uuid")
        .bind(&me)
        .execute(&state.pg)
        .await
        .unwrap();
    let up = upload(&state, &cookie, "me.webp", "image/webp", b"webp").await;
    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": up })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["picture"], "https://google.test/me.png");
    let (_, session) = call_json(&state, "GET", "/api/auth/session", Some(&cookie), None).await;
    assert_eq!(session["user"]["picture"], "https://google.test/me.png");
}

/// AE4: a Google sign-in after the upload rewrites `picture`, and the
/// session's picture is still the uploaded photo.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_google_sign_in_after_an_upload_keeps_the_photo() {
    let state = live_app_state().await;
    let (me, cookie, email) = member(&state).await;
    let up = upload(&state, &cookie, "me.jpg", "image/jpeg", b"jpeg").await;
    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "avatarUploadId": up })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let row = upsert_user(
        &state.pg,
        &Identity {
            sub: format!("{PREFIX}:google:{}", uuid::Uuid::new_v4()),
            email: Some(email.clone()),
            name: Some("Googler".into()),
            picture: Some("https://google.test/new.png".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(row.0, me, "the sign-in links to the same person");
    assert_eq!(row.4.as_deref(), Some(avatar_url(&me, &up).as_str()));
    let stored: Option<String> =
        sqlx::query_scalar("select picture from users where id = $1::uuid")
            .bind(&me)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(
        stored.as_deref(),
        Some("https://google.test/new.png"),
        "the provider picture is still refreshed underneath"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn status_is_bounded_and_round_trips_through_the_directory() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "statusEmoji": "x".repeat(17) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "statusText": "x".repeat(101) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "statusEmoji": "📅", "statusText": " In a meeting " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["statusEmoji"], "📅");
    assert_eq!(body["user"]["statusText"], "In a meeting");

    let (status, dir) = call_json(&state, "GET", "/api/users", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let mine = dir["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == me.as_str())
        .expect("I am in the directory");
    assert_eq!(mine["statusEmoji"], "📅");
    assert_eq!(mine["statusText"], "In a meeting");
    let (_, session) = call_json(&state, "GET", "/api/auth/session", Some(&cookie), None).await;
    assert_eq!(session["user"]["statusEmoji"], "📅");
}

/// AE3: after a presence ping the directory shows the person online; one who
/// never pinged is offline.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_presence_ping_marks_one_person_online() {
    let state = live_app_state().await;
    let (pinger, cookie, _) = member(&state).await;
    let (silent, _, _) = member(&state).await;
    let (status, body) = call_json(&state, "PUT", "/api/me/presence", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "ok": true }));

    let (_, dir) = call_json(&state, "GET", "/api/users", Some(&cookie), None).await;
    let online = |id: &str| {
        dir["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == id)
            .map(|u| u["online"].clone())
    };
    assert_eq!(online(&pinger), Some(json!(true)));
    assert_eq!(online(&silent), Some(json!(false)));

    let (status, _) = call_json(&state, "PUT", "/api/me/presence", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

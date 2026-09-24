// Live-DB + live-Redis proof for the uploads body cap. The bug this file
// pins, reported across the fleet 2026-09-03 ("attachments are broken
// basically everywhere"): axum ships a hidden DEFAULT 2 MB DefaultBodyLimit
// that caps the Multipart extractor mid-stream, so every upload larger than
// 2 MB died as a Malformed — the route's own 400 `no file`, blaming the
// client for a body the server never let finish — while the handler's real
// 25 MB ceiling sat unreachable above it. The fix gives the route an explicit
// limit one envelope beyond the form read's own cap; these assertions drive
// the REAL router (the exact stack the process serves) through a 3 MB
// upload → download round trip, and prove the 413 that answers an oversized
// body is the handler's house message, not the extractor's abort.
//
// The third proof here covers the KB-body read seam: an upload id embedded
// in a kb doc's body is readable by whoever can read the doc (TALA-2's
// inline attachments ride this path). The seam (uploads'
// KB_DOC_ALLOWS_READ) shipped unset after the uploads-crate extraction and
// answered 404 for every non-owner reader — the same disease as
// CONVERSATION_OWNER — so this test wires it the way boot does and drives
// the real route.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test it uploads_live:: -- --ignored

use axum::body::Body;
use axum::http::Request;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use talaria_api::uploads::MAX_BYTES;

use crate::support::wire_boot_seams;
use tower::ServiceExt; // oneshot

fn live_config() -> Config {
    Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL (source ui/.env)"),
        std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)"),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles")
}

async fn app_state() -> AppState {
    let cfg = live_config();
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

/// A member with the session minted the way the auth routes do, so the
/// router's own cookie → Redis → perm-gate path runs for real.
async fn minted_member(state: &AppState) -> (String, String) {
    let sub = format!("uploads-live:{}", uuid::Uuid::new_v4());
    sqlx::query("insert into users (sub) values ($1)")
        .bind(&sub)
        .execute(&state.pg)
        .await
        .unwrap();
    let id: String = sqlx::query_scalar("select id::text from users where sub = $1")
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
        role: "member".into(),
        provider: "password".into(),
    };
    let sid = create_session(state, &user).await.expect("session mints");
    (id, sid)
}

/// Deterministic bytes — a full-download comparison needs the same pattern
/// on the way back, not zeros the transport could shortcut.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// A browser-shaped multipart/form-data body: one `file` part, its bytes
/// exactly `payload`.
fn multipart_body(boundary: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// THE SEAM'S PROOF, as a wire contract: an upload embedded in a KB doc's
/// body serves 200 to a member who can read that doc and 404 to one who
/// cannot. The author uploads, the doc's body carries the id, and the two
/// readers differ only in doc visibility — `org` answers any member,
/// `private` with no grant answers nobody but the owner (and the admin,
/// who is not minted here).
///
/// The seam is wired exactly the way `register_all` wires it in production
/// (support::wire_boot_seams — test binaries never run the arming path).
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_kb_body_embedded_upload_reads_as_the_doc_does() {
    wire_boot_seams();
    let state = app_state().await;
    let (author_id, author_sid) = minted_member(&state).await;
    let author_cookie = format!("talaria_session={author_sid}");

    // The upload, made by the author the way every upload is.
    let bytes = payload(1024);
    let body = multipart_body("talaria-live", "embedded.bin", &bytes);
    let res = routes::router(state.clone())
        .oneshot(
            Request::post("/api/uploads")
                .header("content-type", "multipart/form-data; boundary=talaria-live")
                .header("cookie", &author_cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the author's upload is a normal upload");
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let att: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = att["id"].as_str().expect("id on the wire").to_string();

    // A space any member can see, two docs whose bodies embed the id: one
    // org-visible (every member reads it), one private with no grant (only
    // the owner reads it).
    let space: String = sqlx::query_scalar(
        "insert into kb_spaces (name) values ('uploads-live') returning id::text",
    )
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let doc_body = format!("[embedded.bin](attachment://{id}?s=1024&m=application%2Fbin)");
    let org_doc: String = sqlx::query_scalar(
        "insert into kb_docs (space_id, title, body, kind, created_by, updated_by, owner_user_id, visibility) \
         values ($1::uuid, 'org doc', $2, 'human', $3, $3, $3::uuid, 'org') returning id::text",
    )
    .bind(&space)
    .bind(&doc_body)
    .bind(&author_id)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (priv_doc,): (String,) = sqlx::query_as(
        "insert into kb_docs (space_id, title, body, kind, created_by, updated_by, owner_user_id, visibility, perms_inherited) \
         values ($1::uuid, 'private doc', $2, 'human', $3, $3, $3::uuid, 'private', false) returning id::text",
    )
    .bind(&space)
    .bind(&doc_body)
    .bind(&author_id)
    .fetch_one(&state.pg)
    .await
    .unwrap();

    // Two more members: identical in every way, differing only in what
    // they can read. The org doc is readable by both (org visibility); the
    // private doc by neither. The upload id appears in BOTH bodies, so
    // reach through the org doc must answer 200 and the private-only path
    // must answer 404 — the seam's exact decision.
    let (_, reader_sid) = minted_member(&state).await;
    let reader_cookie = format!("talaria_session={reader_sid}");
    let res = routes::router(state.clone())
        .oneshot(
            Request::get(format!("/api/uploads/{id}"))
                .header("cookie", &reader_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "the org-visible doc's embedded upload serves to a member"
    );

    // The same id in a private doc: unreachable without a grant — the
    // seam must not have short-circuited into "any doc match grants". The
    // private doc is the ONLY doc for this second upload, so mint a fresh
    // one to keep the two probes independent.
    let bytes2 = payload(512);
    let body2 = multipart_body("talaria-live", "private-embedded.bin", &bytes2);
    let res = routes::router(state.clone())
        .oneshot(
            Request::post("/api/uploads")
                .header("content-type", "multipart/form-data; boundary=talaria-live")
                .header("cookie", &author_cookie)
                .body(Body::from(body2))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let att2: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id2 = att2["id"].as_str().expect("id on the wire").to_string();
    sqlx::query("update kb_docs set body = $1 where id = $2::uuid")
        .bind(format!(
            "[p.bin](attachment://{id2}?s=512&m=application%2Fbin)"
        ))
        .bind(&priv_doc)
        .execute(&state.pg)
        .await
        .unwrap();
    let (_, stranger_sid) = minted_member(&state).await;
    let stranger_cookie = format!("talaria_session={stranger_sid}");
    let res = routes::router(state.clone())
        .oneshot(
            Request::get(format!("/api/uploads/{id2}"))
                .header("cookie", &stranger_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        404,
        "a private doc's embedded upload stays private to its readers"
    );

    // Cleanup: both uploads, both docs, the space, all three users.
    for upload_id in [&id, &id2] {
        let path: Option<String> =
            sqlx::query_scalar("select path from uploads where id = $1::uuid")
                .bind(upload_id)
                .fetch_one(&state.pg)
                .await
                .ok();
        sqlx::query("delete from uploads where id = $1::uuid")
            .bind(upload_id)
            .execute(&state.pg)
            .await
            .unwrap();
        if let Some(p) = path.filter(|p| !p.contains("://")) {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
    for doc_id in [&org_doc, &priv_doc] {
        sqlx::query("delete from kb_docs where id = $1::uuid")
            .bind(doc_id)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    sqlx::query("delete from kb_spaces where id = $1::uuid")
        .bind(&space)
        .execute(&state.pg)
        .await
        .unwrap();
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&author_id)
        .execute(&state.pg)
        .await
        .unwrap();
}

/// THE REPORT, as a wire contract: a 3 MB upload — half again over the
/// extractor's old 2 MB grave line, well under the real 25 MB ceiling — is
/// accepted, recorded, and served back byte-identical under its own name.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_big_upload_round_trips_with_its_name() {
    let state = app_state().await;
    let (user_id, sid) = minted_member(&state).await;
    let cookie = format!("talaria_session={sid}");

    let bytes = payload(3 * 1024 * 1024);
    let body = multipart_body("talaria-live", "big.bin", &bytes);
    let res = routes::router(state.clone())
        .oneshot(
            Request::post("/api/uploads")
                .header("content-type", "multipart/form-data; boundary=talaria-live")
                .header("cookie", &cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    // Pre-fix this was the 400 `no file` — the extractor's abort wearing the
    // route's refusal. The ceiling is the handler's, and 3 MB is under it.
    assert_eq!(res.status(), 200, "a 3 MB upload is a normal upload");
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let att: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let id = att["id"].as_str().expect("id on the wire").to_string();
    assert_eq!(att["filename"], "big.bin");
    assert_eq!(att["size"], 3 * 1024 * 1024);

    // The download keeps what the upload claimed: the bytes, the name, and
    // the sandbox belt serve_upload buckles on (the belt the dev proxy used
    // to strip — this asserts the api side of that contract).
    let res = routes::router(state.clone())
        .oneshot(
            Request::get(format!("/api/uploads/{id}"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers().get("content-disposition").unwrap(),
        "attachment; filename=\"big.bin\""
    );
    assert_eq!(
        res.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        res.headers().get("content-security-policy").unwrap(),
        "default-src 'none'; sandbox"
    );
    let served = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&served[..], &bytes[..], "bytes round-trip untouched");

    // Cleanup: the row, the blob when it lives on this disk (an instance in
    // MinIO mode simply leaves one test object behind), the user.
    let path: Option<String> = sqlx::query_scalar("select path from uploads where id = $1::uuid")
        .bind(&id)
        .fetch_one(&state.pg)
        .await
        .ok();
    sqlx::query("delete from uploads where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await
        .unwrap();
    if let Some(p) = path.filter(|p| !p.contains("://")) {
        let _ = tokio::fs::remove_file(p).await;
    }
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&user_id)
        .execute(&state.pg)
        .await
        .unwrap();
}

/// The refusal above the ceiling is the handler's own sentence. The probe
/// sits in the WINDOW between the form read's cap (MAX_BYTES + one envelope)
/// and the route's limit (one envelope beyond that) — the band where the
/// handler's running total, not the extractor's abort, is the check an
/// unsized oversized body meets. (A body beyond the route limit answers the
/// extractor's 400; only a chunked sender can even get there, since every
/// sized request is refused by the content-length pre-check first.)
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_ceiling_that_answers_is_the_handlers_own() {
    let state = app_state().await;
    let (user_id, sid) = minted_member(&state).await;

    // 25 MB + 96 KiB of file: past the handler's cap, under the route's.
    let bytes = payload(MAX_BYTES + 96 * 1024);
    let body = multipart_body("talaria-live", "huge.bin", &bytes);
    let res = routes::router(state.clone())
        .oneshot(
            Request::post("/api/uploads")
                .header("content-type", "multipart/form-data; boundary=talaria-live")
                .header("cookie", format!("talaria_session={sid}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let err: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        err["error"], "file too large (max 25 MB)",
        "the house message, not an extractor abort"
    );

    sqlx::query("delete from users where id = $1::uuid")
        .bind(&user_id)
        .execute(&state.pg)
        .await
        .unwrap();
}

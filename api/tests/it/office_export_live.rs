// Live proof for the Office surface: the artifact export route
// (GET /api/artifacts/{id}/export?format=docx|xlsx) and the attachment path
// that turns an uploaded .docx into prompt text.
//
// WHY THESE LIVE HERE rather than in talaria-office: that crate's own tests
// prove its writer and its reader against each other, in memory. What they
// cannot see is the wire — whether the route hands a browser bytes that are
// actually a document, whether the headers name it (a .docx served as
// application/json downloads as a broken file), and whether the gate in
// front of it is the artifact read gate rather than a second, looser one
// invented for the export. So every assertion here is a round trip through
// the REAL router and a minted session: markdown in the row → the route →
// zip → XML → text again.
//
// The refusals are half the contract. A format the artifact's kind cannot
// take, and a kind with no document at all, are 400s with a sentence — the
// route must not answer 200 with an empty file, and must not 500.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it office_export_live:: -- --ignored

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{Value, json};
use talaria_office::OfficeKind;
use talaria_state::AppState;
use tower::ServiceExt; // oneshot

use crate::support::{
    call_bytes, call_json, live_app_state, minted_member_with_email, sweep_user_rows,
};

const PREFIX: &str = "office-export-live";

/// The two OOXML types the export answers with. Spelled here because the
/// assertion is byte-exact: these strings are what a browser matches to
/// decide whether it has a Word document or a download of unknown stuff.
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// A member with a live session, under this file's sweepable `sub` prefix.
async fn member(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@office-export-live.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

/// A title no other run shares, with only `[a-z0-9-]` in it — the
/// Content-Disposition assertion below reads the title back through the
/// route's filename sanitiser, so the fixture must not be the thing that
/// sanitiser changes.
fn title(tag: &str) -> String {
    format!(
        "office-live-{tag}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// An artifact row owned by `owner`, written straight to the table.
///
/// WHY SQL instead of POST /api/artifacts: the create route carries its own
/// gate (`artifacts.create`, which an org's defaults can switch off) plus
/// folder and attribution machinery, none of which is what these tests
/// drive. The row it would write is this row — the table's defaults are
/// kind 'doc', visibility 'private', edit_policy 'owner', which is also the
/// private-by-default artifact the stranger test needs.
async fn artifact(state: &AppState, owner: &str, kind: &str, title: &str, body: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into artifacts (kind, title, body, owner_user_id, created_by, updated_by) \
         values ($1, $2, $3, $4::uuid, $5, $5) returning id::text",
    )
    .bind(kind)
    .bind(title)
    .bind(body)
    .bind(owner)
    .bind(PREFIX)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    id
}

/// Leftovers from a previous failed run, before this one asserts anything.
async fn sweep(state: &AppState, tag: &str) {
    sqlx::query("delete from artifacts where title like $1")
        .bind(format!("office-live-{tag}-%"))
        .execute(&state.pg)
        .await
        .unwrap();
    sweep_user_rows(&state.pg, PREFIX, tag).await;
}

async fn drop_rows(state: &AppState, artifact_id: &str, user_id: &str) {
    sqlx::query("delete from artifacts where id = $1::uuid")
        .bind(artifact_id)
        .execute(&state.pg)
        .await
        .unwrap();
    sqlx::query("delete from users where id = $1::uuid")
        .bind(user_id)
        .execute(&state.pg)
        .await
        .unwrap();
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// `extract_text` with the failure spelled through OfficeError's Display: a
/// panic here means the route handed a browser something that is not the
/// document it claimed, which is the one failure a status code cannot show.
fn text_of(bytes: &[u8], kind: OfficeKind, what: &str) -> String {
    match talaria_office::extract_text(bytes, kind) {
        Ok(text) => text,
        Err(e) => panic!("the exported {what} must read back as text: {e}"),
    }
}

/// A doc artifact's markdown comes back as a real .docx: the right type, a
/// filename the browser will save under, zip bytes, and — the round trip —
/// the heading, the paragraph (its **bold** now a bold RUN, so the text is
/// the words without the asterisks) and the list item all present in the
/// text read back out of the file.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_doc_exports_as_a_docx_that_reads_back() {
    let state = live_app_state().await;
    sweep(&state, "docx").await;
    let (me, cookie) = member(&state, "docx").await;
    let title = title("docx");
    let markdown = "# Title\n\nA paragraph with **bold**.\n\n- one\n- two";
    let id = artifact(&state, &me, "doc", &title, markdown).await;

    let (status, headers, bytes) = call_bytes(
        &state,
        "GET",
        &format!("/api/artifacts/{id}/export?format=docx"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the owner's own doc exports: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(
        header(&headers, "content-type"),
        DOCX_MIME,
        "a Word document says so on the wire"
    );
    let disposition = header(&headers, "content-disposition");
    assert!(
        disposition.starts_with("attachment"),
        "an export downloads, never renders inline: {disposition}"
    );
    assert!(
        disposition.contains(&format!("{title}.docx")),
        "the download is named after the artifact: {disposition}"
    );
    assert!(
        bytes.starts_with(b"PK"),
        "an OOXML document is a zip — these bytes are not"
    );

    let text = text_of(&bytes, OfficeKind::Docx, "docx");
    for want in ["Title", "A paragraph with bold.", "one"] {
        assert!(
            text.contains(want),
            "the exported docx lost {want:?}; it read back as: {text}"
        );
    }

    drop_rows(&state, &id, &me).await;
}

/// A sheet artifact's JSON grid comes back as a real .xlsx, every cell
/// value in it — the string cells as inline strings, `1` and `2.5` as
/// numbers, which is the whole point of exporting a sheet as a sheet rather
/// than as a markdown table.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_sheet_exports_as_an_xlsx_that_reads_back() {
    let state = live_app_state().await;
    sweep(&state, "xlsx").await;
    let (me, cookie) = member(&state, "xlsx").await;
    let title = title("xlsx");
    let id = artifact(&state, &me, "sheet", &title, r#"[["a","1"],["b","2.5"]]"#).await;

    let (status, headers, bytes) = call_bytes(
        &state,
        "GET",
        &format!("/api/artifacts/{id}/export?format=xlsx"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the owner's own sheet exports: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(
        header(&headers, "content-type"),
        XLSX_MIME,
        "a workbook says so on the wire"
    );
    assert!(
        header(&headers, "content-disposition").contains(&format!("{title}.xlsx")),
        "the download is named after the artifact"
    );
    assert!(bytes.starts_with(b"PK"), "an OOXML workbook is a zip");

    let text = text_of(&bytes, OfficeKind::Xlsx, "xlsx");
    for want in ["a", "1", "b", "2.5"] {
        assert!(
            text.contains(want),
            "the exported xlsx lost {want:?}; it read back as: {text}"
        );
    }
    // Row order, not just presence: all four values survive a transposed or
    // re-sorted grid, so read the rows themselves. Past the "## <sheet>"
    // heading the sheet is its rows, cells tab-separated.
    let rows: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty() && !l.starts_with("##"))
        .collect();
    assert_eq!(
        rows,
        vec!["a\t1", "b\t2.5"],
        "the grid read back out of order (or reshaped): {text}"
    );

    drop_rows(&state, &id, &me).await;
}

/// The refusals. A doc has no grid, so xlsx is not a format it can take; pdf
/// is not a format this route writes at all; and a `file` artifact — bytes
/// in storage, no document body — has nothing to export in either format.
/// All three are 400s that say why, never a 200 with an empty file.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_format_the_kind_cannot_take_is_refused() {
    let state = live_app_state().await;
    sweep(&state, "refuse").await;
    let (me, cookie) = member(&state, "refuse").await;
    let doc = artifact(&state, &me, "doc", &title("refuse"), "# just prose").await;
    let file = artifact(&state, &me, "file", &title("refuse"), "").await;

    for (path, why) in [
        (
            format!("/api/artifacts/{doc}/export?format=xlsx"),
            "a doc has no grid to put in a workbook",
        ),
        (
            format!("/api/artifacts/{doc}/export?format=pdf"),
            "pdf is not a format this route writes",
        ),
        (
            format!("/api/artifacts/{file}/export?format=docx"),
            "a file artifact has no document body",
        ),
    ] {
        let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {path}");
        assert!(
            body["error"].is_string() || body["message"].is_string(),
            "the refusal carries a sentence, not an empty envelope: {body}"
        );
    }

    // The file-kind sentence is the one the design fixes, so pin the words
    // (either house envelope — `error` alone, or `error` code + `message`).
    let (_, _, raw) = call_bytes(
        &state,
        "GET",
        &format!("/api/artifacts/{file}/export?format=docx"),
        Some(&cookie),
        None,
    )
    .await;
    let said = String::from_utf8_lossy(&raw);
    assert!(
        said.contains("has no document to export"),
        "the file-kind refusal says what is missing: {said}"
    );

    for id in [&doc, &file] {
        sqlx::query("delete from artifacts where id = $1::uuid")
            .bind(id)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&me)
        .execute(&state.pg)
        .await
        .unwrap();
}

/// The gate is the artifact's own read gate, not a new one. A private
/// artifact belongs to its owner; another member asking for its bytes gets
/// the same answer the Google export route gives a reader who cannot read
/// it — 403 `forbidden` (404 is reserved for an id that is not a row at
/// all). An export that answered 200 here would be a read-permission bypass
/// wearing a new file extension.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_stranger_cannot_export_a_private_artifact() {
    let state = live_app_state().await;
    sweep(&state, "stranger").await;
    let (owner, _owner_cookie) = member(&state, "stranger").await;
    let (stranger, stranger_cookie) = member(&state, "stranger").await;
    let id = artifact(
        &state,
        &owner,
        "doc",
        &title("stranger"),
        "# private\n\nnot for them",
    )
    .await;

    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/artifacts/{id}/export?format=docx"),
        Some(&stranger_cookie),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the export answers the read gate's refusal: {body}"
    );
    assert_eq!(body["error"], "forbidden");

    drop_rows(&state, &id, &owner).await;
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&stranger)
        .execute(&state.pg)
        .await
        .unwrap();
}

/// A browser-shaped multipart/form-data body: one `file` part, the mime the
/// caller declares.
fn multipart_body(boundary: &str, filename: &str, mime: &str, bytes: &[u8]) -> Vec<u8> {
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
    body
}

/// The other end of the surface: a .docx a member ATTACHES becomes prompt
/// text. The upload goes through the real route (so the row, the blob and
/// the attachment object are the ones the app makes), and the assertion is
/// on `attachment_text_blocks` — the one function every transcript builder
/// calls. Before this surface, a .docx matched no TEXT_MIME and contributed
/// nothing: the model was told a file was attached and never saw a word of
/// it.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_docx_attachment_becomes_prompt_text() {
    let state = live_app_state().await;
    sweep(&state, "attach").await;
    let (me, cookie) = member(&state, "attach").await;

    let paragraph = "The paragraph that must reach the model.";
    let docx = talaria_office::markdown_to_docx("Brief", &format!("# Brief\n\n{paragraph}"));
    let body = multipart_body("talaria-office-live", "brief.docx", DOCX_MIME, &docx);
    let res = talaria_api::routes::router(state.clone())
        .oneshot(
            Request::post("/api/uploads")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=talaria-office-live",
                )
                .header("cookie", &cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "a .docx is an ordinary upload");
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let attachment: Value = serde_json::from_slice(&raw).unwrap();
    let upload_id = attachment["id"]
        .as_str()
        .expect("id on the wire")
        .to_string();

    // The attachment object verbatim, the way a message carries it.
    let sb = state.secretbox().await.unwrap_or_default();
    let blocks =
        talaria_uploads::attachment_text_blocks(&state.pg, &sb, &json!([attachment]), 3).await;
    assert!(
        blocks.contains("brief.docx"),
        "the block names the file it came from: {blocks}"
    );
    assert!(
        blocks.contains(paragraph),
        "the docx's words never reached the prompt: {blocks}"
    );
    assert!(
        !blocks.contains("PK"),
        "the raw zip leaked into the prompt instead of its text: {blocks}"
    );

    let path: Option<String> = sqlx::query_scalar("select path from uploads where id = $1::uuid")
        .bind(&upload_id)
        .fetch_one(&state.pg)
        .await
        .ok();
    sqlx::query("delete from uploads where id = $1::uuid")
        .bind(&upload_id)
        .execute(&state.pg)
        .await
        .unwrap();
    if let Some(p) = path.filter(|p| !p.contains("://")) {
        let _ = tokio::fs::remove_file(p).await;
    }
    sqlx::query("delete from users where id = $1::uuid")
        .bind(&me)
        .execute(&state.pg)
        .await
        .unwrap();
}

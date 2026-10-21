// Native Google Docs through the Drive convert grant. The caller has already
// resolved the bearer; this crate never touches connections or a documents
// scope. Create and update are multipart uploads (markdown in, a Doc out).
// Read exports native types as markdown and downloads everything else.

use talaria_body::percent_encode;
use talaria_gateway::provider::http;
use talaria_google_errors::GoogleError;

const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/drive/v3/files";
const FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files";

const GOOGLE_DOC: &str = "application/vnd.google-apps.document";
const GOOGLE_NATIVE_PREFIX: &str = "application/vnd.google-apps";
const MARKDOWN: &str = "text/markdown";

/// What `get_doc_with_token` will pull for a non-native file it reads as
/// text: the same 25 MiB the upload plane admits (`talaria_uploads::MAX_BYTES`,
/// which this leaf crate does not depend on). Routes that hand bytes back
/// pass that limit themselves.
const DOC_READ_CAP_BYTES: usize = 25 * 1024 * 1024;

/// A Doc Drive just created or replaced, in wire order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDoc {
    pub id: String,
    pub url: String,
    pub name: String,
    pub mime_type: String,
}

/// A file pulled back as markdown. `title` is Drive's `name`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocContent {
    pub id: String,
    pub title: String,
    pub url: String,
    pub mime_type: String,
    pub markdown: String,
}

/// One full-text hit. `url` is `webViewLink`, or the Docs editor when Google
/// omitted the link.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub url: String,
    pub modified_time: Option<String>,
}

/// Create-upload URL. Query order is the Drive convert contract.
pub(crate) fn create_upload_url() -> String {
    format!(
        "{UPLOAD_ENDPOINT}?uploadType=multipart&supportsAllDrives=true&fields=id,webViewLink,name,mimeType"
    )
}

/// Content-replace upload URL. `file_id` is a path segment, so it is
/// percent-encoded.
pub(crate) fn update_upload_url(file_id: &str) -> String {
    format!(
        "{UPLOAD_ENDPOINT}/{}?uploadType=multipart&supportsAllDrives=true&fields=id,webViewLink,name,mimeType",
        percent_encode(file_id)
    )
}

pub(crate) fn doc_meta_url(file_id: &str) -> String {
    format!(
        "{FILES_ENDPOINT}/{}?fields=id,name,mimeType,webViewLink&supportsAllDrives=true",
        percent_encode(file_id)
    )
}

pub(crate) fn export_markdown_url(file_id: &str) -> String {
    format!(
        "{FILES_ENDPOINT}/{}/export?mimeType=text/markdown",
        percent_encode(file_id)
    )
}

pub(crate) fn media_url(file_id: &str) -> String {
    format!(
        "{FILES_ENDPOINT}/{}?alt=media&supportsAllDrives=true",
        percent_encode(file_id)
    )
}

/// `files.list` URL for a full-text search. Single quotes in `q` are doubled
/// (Drive's string-literal escape). The clause is percent-encoded because it
/// is interpolated; page size is clamped to 1..=50.
pub(crate) fn fulltext_query(q: &str, page_size: i64) -> String {
    let escaped = q.replace('\'', "''");
    let page = page_size.clamp(1, 50);
    let clause = format!("fullText contains '{escaped}' and trashed = false");
    format!(
        "{FILES_ENDPOINT}?q={}&pageSize={page}&supportsAllDrives=true&includeItemsFromAllDrives=true&fields=files(id,name,mimeType,modifiedTime,webViewLink)",
        percent_encode(&clause)
    )
}

/// Missing or blank `webViewLink` → the Docs editor for `id`.
pub(crate) fn web_view_or_docs_edit(id: &str, web_view_link: Option<&str>) -> String {
    match web_view_link.filter(|link| !link.is_empty()) {
        Some(link) => link.to_string(),
        None => format!("https://docs.google.com/document/d/{id}/edit"),
    }
}

/// Multipart/related body. The boundary is the first 16 hex chars of the
/// metadata JSON: deterministic for a given export, no RNG. Copied from the
/// Drive export builder (non-base64 media, `text/markdown`).
pub(crate) fn build_doc_multipart(metadata_json: &str, markdown: &str) -> (Vec<u8>, String) {
    let hex: String = metadata_json
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let prefix = hex.get(..16).unwrap_or(hex.as_str());
    let boundary = format!("talaria-drive-{prefix}");
    let head = format!(
        "--{boundary}\r\n\
         Content-Type: application/json; charset=UTF-8\r\n\r\n\
         {metadata_json}\r\n\
         --{boundary}\r\n\
         Content-Type: {MARKDOWN}\r\n\r\n"
    );
    let tail = format!("\r\n--{boundary}--");
    let mut body = Vec::with_capacity(head.len() + markdown.len() + tail.len());
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(markdown.as_bytes());
    body.extend_from_slice(tail.as_bytes());
    (body, boundary)
}

#[derive(serde::Serialize)]
struct CreateMetadata<'a> {
    name: &'a str,
    #[serde(rename = "mimeType")]
    mime_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<&'a str>>,
}

#[derive(serde::Serialize)]
struct UpdateMetadata<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

fn doc_name(title: &str) -> &str {
    if title.is_empty() { "Untitled" } else { title }
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

fn google_doc_of(file: &serde_json::Value, fallback_name: &str) -> GoogleDoc {
    let id = str_field(file, "id");
    let name = file
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_name.to_string());
    let mime_type = file
        .get("mimeType")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| GOOGLE_DOC.to_string());
    let url = web_view_or_docs_edit(&id, file.get("webViewLink").and_then(|v| v.as_str()));
    GoogleDoc {
        id,
        url,
        name,
        mime_type,
    }
}

async fn failed_status(res: reqwest::Response, what: &str) -> GoogleError {
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    tracing::error!(status = %status, "{what}");
    GoogleError::Failed(format!("{what}: {status} {text}"))
}

async fn upload_multipart(
    patch: bool,
    url: &str,
    token: &str,
    metadata_json: &str,
    markdown: &str,
    fallback_name: &str,
    what: &str,
) -> Result<GoogleDoc, GoogleError> {
    let (body, boundary) = build_doc_multipart(metadata_json, markdown);
    let client = http();
    let req = if patch {
        client.patch(url)
    } else {
        client.post(url)
    };
    let res = req
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("{what} request: {e}")))?;
    if !res.status().is_success() {
        let failed = format!("{what} failed");
        return Err(failed_status(res, &failed).await);
    }
    let file: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("{what} body: {e}")))?;
    Ok(google_doc_of(&file, fallback_name))
}

/// Create a native Doc from markdown. `folder_id` (folder or Shared Drive),
/// when non-empty, is the parent so the file is team-owned there. An empty
/// title is stored as "Untitled".
#[tracing::instrument(skip(token, markdown))]
pub async fn create_doc_with_token(
    token: &str,
    title: &str,
    markdown: &str,
    folder_id: Option<&str>,
) -> Result<GoogleDoc, GoogleError> {
    let name = doc_name(title);
    let metadata = CreateMetadata {
        name,
        mime_type: GOOGLE_DOC,
        parents: folder_id.filter(|id| !id.is_empty()).map(|id| vec![id]),
    };
    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| GoogleError::Failed(format!("docs metadata encode: {e}")))?;
    upload_multipart(
        false,
        &create_upload_url(),
        token,
        &metadata_json,
        markdown,
        name,
        "docs create",
    )
    .await
}

/// Metadata, then markdown. Native Google types (`application/vnd.google-apps*`)
/// are exported; everything else is the media bytes, lossy-UTF-8.
#[tracing::instrument(skip(token))]
pub async fn get_doc_with_token(token: &str, file_id: &str) -> Result<DocContent, GoogleError> {
    let res = http()
        .get(doc_meta_url(file_id))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs get request: {e}")))?;
    if !res.status().is_success() {
        return Err(failed_status(res, "docs get failed").await);
    }
    let meta: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs get body: {e}")))?;
    let id = {
        let from_drive = str_field(&meta, "id");
        if from_drive.is_empty() {
            file_id.to_string()
        } else {
            from_drive
        }
    };
    let mime_type = str_field(&meta, "mimeType");
    let title = str_field(&meta, "name");
    let url = web_view_or_docs_edit(&id, meta.get("webViewLink").and_then(|v| v.as_str()));
    let markdown = if mime_type.starts_with(GOOGLE_NATIVE_PREFIX) {
        export_markdown(token, file_id).await?
    } else {
        let bytes = read_file_media_with_token(token, file_id, DOC_READ_CAP_BYTES).await?;
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Ok(DocContent {
        id,
        title,
        url,
        mime_type,
        markdown,
    })
}

async fn export_markdown(token: &str, file_id: &str) -> Result<String, GoogleError> {
    let res = http()
        .get(export_markdown_url(file_id))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs export request: {e}")))?;
    if !res.status().is_success() {
        return Err(failed_status(res, "docs export failed").await);
    }
    res.text()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs export body: {e}")))
}

/// Replace the Doc body with markdown. `title: Some` renames; `None` leaves
/// the name alone (metadata omits `name`).
#[tracing::instrument(skip(token, markdown))]
pub async fn update_doc_with_token(
    token: &str,
    file_id: &str,
    markdown: &str,
    title: Option<&str>,
) -> Result<GoogleDoc, GoogleError> {
    let metadata = UpdateMetadata { name: title };
    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| GoogleError::Failed(format!("docs metadata encode: {e}")))?;
    upload_multipart(
        true,
        &update_upload_url(file_id),
        token,
        &metadata_json,
        markdown,
        title.unwrap_or(""),
        "docs update",
    )
    .await
}

/// Full-text search across Drive, including Shared Drives. A blank query is
/// an empty vec and does not call Drive. Page size is clamped to 1..=50.
#[tracing::instrument(skip(token))]
pub async fn search_files_fulltext(
    token: &str,
    query: &str,
    page_size: i64,
) -> Result<Vec<FileHit>, GoogleError> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let res = http()
        .get(fulltext_query(query.trim(), page_size))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs search request: {e}")))?;
    if !res.status().is_success() {
        return Err(failed_status(res, "docs search failed").await);
    }
    let data: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs search body: {e}")))?;
    Ok(data
        .get("files")
        .and_then(|v| v.as_array())
        .map(|files| files.iter().map(file_hit_of).collect())
        .unwrap_or_default())
}

fn file_hit_of(f: &serde_json::Value) -> FileHit {
    let id = str_field(f, "id");
    let mime_type = str_field(f, "mimeType");
    FileHit {
        url: web_view_or_docs_edit(&id, f.get("webViewLink").and_then(|v| v.as_str())),
        name: str_field(f, "name"),
        mime_type,
        modified_time: f
            .get("modifiedTime")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        id,
    }
}

/// Raw media bytes (`alt=media`), at most `max_bytes` of them. Non-2xx and
/// network failures are `GoogleError::Failed` with status and body text
/// when Google answered; a file over the cap — by its `Content-Length`, or
/// by the bytes actually streamed when Google sent no length — is
/// `GoogleError::Failed` too, before the whole of it is in memory. The cap
/// is the caller's: the routes pass the upload limit, so what an agent can
/// pull out of Drive is what a person can push in.
#[tracing::instrument(skip(token))]
pub async fn read_file_media_with_token(
    token: &str,
    file_id: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, GoogleError> {
    let res = http()
        .get(media_url(file_id))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs media request: {e}")))?;
    if !res.status().is_success() {
        return Err(failed_status(res, "docs media failed").await);
    }
    read_capped(res, max_bytes, "docs media").await
}

/// Export a native file as bytes (`export?mimeType=`), under the same cap.
/// The Drive import's PDF arm reads through here.
#[tracing::instrument(skip(token))]
pub async fn export_file_bytes_with_token(
    token: &str,
    file_id: &str,
    mime_type: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, GoogleError> {
    let res = http()
        .get(format!(
            "{FILES_ENDPOINT}/{}/export?mimeType={}",
            percent_encode(file_id),
            percent_encode(mime_type)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive export request: {e}")))?;
    if !res.status().is_success() {
        return Err(failed_status(res, &format!("drive export({mime_type}) failed")).await);
    }
    read_capped(res, max_bytes, "drive export").await
}

/// The body, streamed under a cap. `Content-Length` refuses early; the
/// running total refuses a chunked or lying response at the same line. The
/// one copy: every Google engine that pulls bytes reads through here.
pub async fn read_capped(
    mut res: reqwest::Response,
    max_bytes: usize,
    what: &str,
) -> Result<Vec<u8>, GoogleError> {
    if let Some(len) = res.content_length()
        && len > max_bytes as u64
    {
        return Err(GoogleError::Failed(format!(
            "{what} too large: {len} bytes, the limit is {max_bytes}"
        )));
    }
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| GoogleError::Failed(format!("{what} body: {e}")))?
    {
        if out.len() + chunk.len() > max_bytes {
            return Err(GoogleError::Failed(format!(
                "{what} too large: more than {max_bytes} bytes"
            )));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Who last changed a Drive file, as Drive tells the token holder.
/// `by_token_holder` is Drive's `lastModifyingUser.me`; `None` when Drive
/// sent no modifier (a file nobody has edited since creation reports its
/// creator, so None is rare).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastModifier {
    pub modified_time: Option<String>,
    pub by_token_holder: Option<bool>,
    pub email: Option<String>,
}

/// `modifiedTime` plus `lastModifyingUser` for one file. The agent-doc
/// routes ask this before an immediate full-body replace: a doc a human has
/// edited since is no longer the agent's scratch file.
#[tracing::instrument(skip(token))]
pub async fn last_modifier_with_token(
    token: &str,
    file_id: &str,
) -> Result<LastModifier, GoogleError> {
    let res = http()
        .get(format!(
            "{FILES_ENDPOINT}/{}?supportsAllDrives=true&fields=modifiedTime,lastModifyingUser(emailAddress,me)",
            percent_encode(file_id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs modifier request: {e}")))?;
    if !res.status().is_success() {
        return Err(failed_status(res, "docs modifier failed").await);
    }
    let meta: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("docs modifier body: {e}")))?;
    Ok(last_modifier_from(&meta))
}

pub(crate) fn last_modifier_from(meta: &serde_json::Value) -> LastModifier {
    let user = meta.get("lastModifyingUser");
    LastModifier {
        modified_time: meta
            .get("modifiedTime")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        by_token_holder: user.and_then(|u| u.get("me")).and_then(|v| v.as_bool()),
        email: user
            .and_then(|u| u.get("emailAddress"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// How far Drive's `modifiedTime` may run past the agent's own write stamp
/// before it counts as somebody else's edit. The stamp is taken after Drive
/// answers the agent's write, so an honest revision is never later than it
/// by more than clock skew; the allowance absorbs that skew without hiding a
/// person's edit, which lands seconds to days later, not milliseconds.
pub const AGENT_WRITE_SKEW_MS: i64 = 5_000;

/// Whether a person edited the doc after the agent's last write, judged from
/// Drive's `modifiedTime` against the stamp Talaria took when the agent last
/// wrote it. This is the check `lastModifyingUser.me` cannot make: the owner
/// editing their own assistant's doc is the token holder, so Drive reports
/// `me: true` for them and for the agent alike, but the clock does not lie —
/// a revision later than the agent's own write was not the agent's. Unknown
/// on either side (Drive sent no time, or the row was never stamped) is not
/// evidence, so it answers `false` and the `me` flag alone decides.
pub fn modified_after_agent(modified_time: Option<&str>, last_agent_write_ms: Option<i64>) -> bool {
    let (Some(modified), Some(stamp)) =
        (modified_time.and_then(rfc3339_to_ms), last_agent_write_ms)
    else {
        return false;
    };
    modified > stamp + AGENT_WRITE_SKEW_MS
}

/// An RFC 3339 instant (`2026-10-20T10:00:00.123Z`, any fraction length, `Z`
/// or a `±HH:MM` offset) as epoch milliseconds — the form Drive's
/// `modifiedTime` takes. Anything else is `None`, never a guess.
pub fn rfc3339_to_ms(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let b = raw.as_bytes();
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = raw.get(from..to)?;
        if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse::<i64>().ok()
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut i = 19;
    let mut millis = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        // Three digits of fraction are milliseconds; fewer are scaled up,
        // more are truncated.
        let frac = &raw[start..i];
        let scaled: String = frac.chars().chain("000".chars()).take(3).collect();
        millis = scaled.parse::<i64>().ok()?;
    }
    let offset_s = match b.get(i) {
        Some(b'Z') | Some(b'z') if i + 1 == b.len() => 0i64,
        Some(sign @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
            let oh = num(i + 1, i + 3)?;
            let om = num(i + 4, i + 6)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let total = oh * 3600 + om * 60;
            if *sign == b'-' { -total } else { total }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_s;
    Some(secs * 1000 + millis)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`), so the parser above needs no calendar crate.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_boundary_is_deterministic_and_contains_the_markdown() {
        let meta = r#"{"name":"Notes","mimeType":"application/vnd.google-apps.document"}"#;
        let markdown = "# Hello\n\nquoted body";
        let (once, boundary) = build_doc_multipart(meta, markdown);
        let (twice, boundary_again) = build_doc_multipart(meta, markdown);
        assert_eq!(boundary, boundary_again);
        assert_eq!(once, twice);
        assert!(boundary.starts_with("talaria-drive-"));
        assert_eq!(boundary.len(), "talaria-drive-".len() + 16);
        let text = String::from_utf8(once).expect("multipart is utf-8");
        assert!(text.contains(markdown));
        assert!(text.contains(&format!("--{boundary}")));
        assert!(text.contains("Content-Type: text/markdown"));
        // Boundary is a function of the metadata only.
        let (_, other) = build_doc_multipart(meta, "different");
        assert_eq!(boundary, other);
    }

    #[test]
    fn fulltext_query_escapes_quotes_and_clamps_page_size() {
        let low = fulltext_query("jon's notes", 0);
        assert!(low.contains("jon''s"), "{low}");
        assert!(low.contains("pageSize=1"), "{low}");
        assert!(low.contains("supportsAllDrives=true"), "{low}");
        assert!(low.contains("includeItemsFromAllDrives=true"), "{low}");
        let high = fulltext_query("a'b", 500);
        assert!(high.contains("a''b"), "{high}");
        assert!(high.contains("pageSize=50"), "{high}");
        let mid = fulltext_query("plain", 10);
        assert!(mid.contains("pageSize=10"), "{mid}");
        assert!(mid.contains("fullText%20contains%20'plain'"), "{mid}");
        assert!(mid.contains("trashed"), "{mid}");
    }

    #[tokio::test]
    async fn empty_search_query_is_empty() {
        let hits = search_files_fulltext("token", "", 10)
            .await
            .expect("blank query does not call Drive");
        assert!(hits.is_empty());
        let blank = search_files_fulltext("token", "   ", 25)
            .await
            .expect("whitespace query does not call Drive");
        assert!(blank.is_empty());
    }

    #[test]
    fn upload_urls_contain_multipart_and_all_drives() {
        for url in [create_upload_url(), update_upload_url("file/id")] {
            assert!(url.contains("uploadType=multipart"), "{url}");
            assert!(url.contains("supportsAllDrives=true"), "{url}");
        }
        assert!(update_upload_url("file/id").contains("file%2Fid"));
        assert!(doc_meta_url("a b").contains("a%20b"));
        assert!(export_markdown_url("a b").contains("mimeType=text/markdown"));
        assert!(media_url("a b").contains("alt=media"));
        assert!(media_url("a b").contains("supportsAllDrives=true"));
    }

    #[test]
    fn missing_web_view_link_builds_the_docs_edit_url() {
        assert_eq!(
            web_view_or_docs_edit("abc", None),
            "https://docs.google.com/document/d/abc/edit"
        );
        assert_eq!(
            web_view_or_docs_edit("abc", Some("")),
            "https://docs.google.com/document/d/abc/edit"
        );
        assert_eq!(
            web_view_or_docs_edit("abc", Some("https://docs.google.com/document/d/abc/edit")),
            "https://docs.google.com/document/d/abc/edit"
        );
    }
    #[test]
    fn the_last_modifier_reads_drives_me_flag() {
        let human = last_modifier_from(&serde_json::json!({
            "modifiedTime": "2026-10-20T10:00:00Z",
            "lastModifyingUser": { "emailAddress": "jon@x.com", "me": false }
        }));
        assert_eq!(human.by_token_holder, Some(false));
        assert_eq!(human.email.as_deref(), Some("jon@x.com"));
        assert_eq!(human.modified_time.as_deref(), Some("2026-10-20T10:00:00Z"));
        let me = last_modifier_from(&serde_json::json!({ "lastModifyingUser": { "me": true } }));
        assert_eq!(me.by_token_holder, Some(true));
        let unknown = last_modifier_from(&serde_json::json!({}));
        assert_eq!(unknown.by_token_holder, None);
    }

    #[test]
    fn drive_timestamps_parse_to_epoch_milliseconds() {
        assert_eq!(rfc3339_to_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_ms("1970-01-01T00:00:00.250Z"), Some(250));
        // Drive's own shape: millisecond fraction, Z.
        assert_eq!(
            rfc3339_to_ms("2026-10-20T10:00:00.123Z"),
            Some(1_792_490_400_123)
        );
        // Fraction lengths other than three are scaled, not misread.
        assert_eq!(
            rfc3339_to_ms("2026-10-20T10:00:00.5Z"),
            Some(1_792_490_400_500)
        );
        assert_eq!(
            rfc3339_to_ms("2026-10-20T10:00:00.123456Z"),
            Some(1_792_490_400_123)
        );
        // An offset moves the instant the other way.
        assert_eq!(
            rfc3339_to_ms("2026-10-20T12:00:00+02:00"),
            Some(1_792_490_400_000)
        );
        assert_eq!(
            rfc3339_to_ms("2026-10-20T08:00:00-02:00"),
            Some(1_792_490_400_000)
        );
        // A leap day and a year boundary land where the calendar says.
        assert_eq!(
            rfc3339_to_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        assert_eq!(rfc3339_to_ms("2000-01-01T00:00:00Z"), Some(946_684_800_000));
    }

    #[test]
    fn a_timestamp_that_is_not_rfc3339_is_not_guessed_at() {
        for bad in [
            "",
            "yesterday",
            "2026-10-20",
            "2026-10-20T10:00:00",
            "2026-10-20T10:00:00.Z",
            "2026-13-20T10:00:00Z",
            "2026-10-20T25:00:00Z",
            "2026-10-20T10:00:00+2:00",
            "2026-10-20T10:00:00Zjunk",
            "1792490400123",
        ] {
            assert_eq!(rfc3339_to_ms(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_revision_after_the_agents_own_write_is_somebody_elses() {
        let stamp = 1_792_490_400_000; // 2026-10-20T10:00:00Z
        // An hour later: a person edited.
        assert!(modified_after_agent(
            Some("2026-10-20T11:00:00.000Z"),
            Some(stamp)
        ));
        // One second past the skew allowance still counts.
        assert!(modified_after_agent(
            Some("2026-10-20T10:00:06.000Z"),
            Some(stamp)
        ));
        // Drive's revision of the agent's own write: at or just before the
        // stamp, or inside the skew allowance after it.
        assert!(!modified_after_agent(
            Some("2026-10-20T09:59:59.900Z"),
            Some(stamp)
        ));
        assert!(!modified_after_agent(
            Some("2026-10-20T10:00:00.000Z"),
            Some(stamp)
        ));
        assert!(!modified_after_agent(
            Some("2026-10-20T10:00:05.000Z"),
            Some(stamp)
        ));
        // Long before the stamp: the agent's write was the latest.
        assert!(!modified_after_agent(
            Some("2026-10-19T10:00:00Z"),
            Some(stamp)
        ));
    }

    #[test]
    fn an_unknown_side_is_not_evidence_of_a_persons_edit() {
        assert!(!modified_after_agent(None, Some(1_792_490_400_000)));
        assert!(!modified_after_agent(Some("2026-10-20T11:00:00Z"), None));
        assert!(!modified_after_agent(None, None));
        // Unparseable is unknown, not "later".
        assert!(!modified_after_agent(Some("soon"), Some(0)));
    }
}

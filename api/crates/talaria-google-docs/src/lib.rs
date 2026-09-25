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
        let bytes = read_file_media_with_token(token, file_id).await?;
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

/// Raw media bytes (`alt=media`). Non-2xx and network failures are
/// `GoogleError::Failed` with status and body text when Google answered.
#[tracing::instrument(skip(token))]
pub async fn read_file_media_with_token(
    token: &str,
    file_id: &str,
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
    res.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| GoogleError::Failed(format!("docs media body: {e}")))
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
}

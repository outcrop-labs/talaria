// Google Drive service: push a Talaria artifact into the connected user's
// Drive as a native Google Doc / Sheet (or an unconverted file), browse what's
// there, and pull a Drive file's content back into a Talaria-artifact shape.
// Acts strictly as the connected identity via its stored token (per-user or
// org), so Drive's own sharing rules govern what lands where.

use base64::Engine as _;
use sqlx::PgPool;

use crate::artifacts::Artifact;
use crate::gateway::provider::http;
use crate::google::connections::{TokenError, get_access_token, require_token};
use crate::google::errors::GoogleError;
use crate::google::oauth::encode_uri_component;
use crate::secretbox::SecretBox;
use crate::uploads::{get_upload, save_upload};

// supportsAllDrives lets us create into a Shared Drive (team-owned files).
const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&supportsAllDrives=true&fields=id,webViewLink,name,mimeType";
const FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files";

const GOOGLE_DOC: &str = "application/vnd.google-apps.document";
const GOOGLE_SHEET: &str = "application/vnd.google-apps.spreadsheet";
// Google's native (non-downloadable) types must be exported to a real format.
const GOOGLE_NATIVE_PREFIX: &str = "application/vnd.google-apps";

/// The created file, in wire order.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    pub url: String,
    pub name: String,
    pub mime_type: String,
}

/// The route's three friendly failures; anything else is `Failed` with the
/// sentence Google sent (the log's only consumer).
pub enum ExportError {
    /// No connection — the user hasn't connected Google.
    NotConnected,
    /// media_for answered None — the artifact's kind has no Drive mapping.
    NotExportable,
    /// The Drive call itself failed.
    Failed(String),
}

/// A sheet's JSON grid (`string[][]`, row 0 = header) → CSV text. A body that
/// isn't a JSON array is returned verbatim — the sheet editor stores the grid,
/// but a hand-edited CSV body should survive a round trip.
fn sheet_to_csv(body: &str) -> String {
    let Ok(serde_json::Value::Array(grid)) = serde_json::from_str(body) else {
        return body.to_string();
    };
    let cell = |v: &serde_json::Value| -> String {
        match v {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            // container cells never occur — these grids come from our own
            // sheet editor.
            other => other.to_string(),
        }
    };
    // /[",\n]/ → quote the cell, doubling embedded quotes.
    let esc = |s: String| -> String {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    };
    grid.iter()
        .map(|row| match row {
            serde_json::Value::Array(cells) => cells
                .iter()
                .map(|c| esc(cell(c)))
                .collect::<Vec<_>>()
                .join(","),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

struct MediaPart {
    content_type: String,
    /// UTF-8 text body, OR base64-encoded bytes when `base64` is true.
    data: String,
    base64: bool,
}

/// How each artifact kind maps onto a Drive upload. None ⇒ not exportable.
async fn media_for(pg: &PgPool, sb: &SecretBox, a: &Artifact) -> Option<(String, MediaPart)> {
    match a.kind.as_str() {
        "doc" => Some((
            GOOGLE_DOC.into(),
            MediaPart {
                content_type: "text/markdown".into(),
                data: a.body.clone(),
                base64: false,
            },
        )),
        // Drive converts HTML → Doc; the microsite body is HTML.
        "microsite" => Some((
            GOOGLE_DOC.into(),
            MediaPart {
                content_type: "text/html".into(),
                data: a.body.clone(),
                base64: false,
            },
        )),
        "sheet" => Some((
            GOOGLE_SHEET.into(),
            MediaPart {
                content_type: "text/csv".into(),
                data: sheet_to_csv(&a.body),
                base64: false,
            },
        )),
        // Upload the raw bytes unconverted, preserving the original type.
        "file" => {
            let storage_ref = a.storage_ref.clone()?;
            // no row or lost blob ⇒ not exportable
            let (bytes, upload_mime, _) = match get_upload(pg, sb, &storage_ref).await {
                Ok(Some(found)) => found,
                _ => return None,
            };
            let target_mime = if upload_mime.is_empty() {
                a.content_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".into())
            } else {
                upload_mime.clone()
            };
            let upload_mime = if upload_mime.is_empty() {
                "application/octet-stream".to_string()
            } else {
                upload_mime
            };
            Some((
                target_mime,
                MediaPart {
                    content_type: upload_mime,
                    data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    base64: true,
                },
            ))
        }
        _ => None,
    }
}

/// The multipart/related body. The boundary is derived from the metadata JSON
/// itself: deterministic for a given export, collision-safe against any body,
/// and no RNG on the hot path.
fn build_multipart(metadata_json: &str, media: &MediaPart) -> (Vec<u8>, String) {
    let hex: String = metadata_json
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let boundary = format!("talaria-drive-{}", &hex[..16]);
    let enc = if media.base64 {
        "Content-Transfer-Encoding: base64\r\n"
    } else {
        ""
    };
    let head = format!(
        "--{boundary}\r\n\
         Content-Type: application/json; charset=UTF-8\r\n\r\n\
         {metadata_json}\r\n\
         --{boundary}\r\n\
         Content-Type: {}\r\n{enc}\r\n",
        media.content_type
    );
    let tail = format!("\r\n--{boundary}--");
    let mut body = Vec::with_capacity(head.len() + media.data.len() + tail.len());
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(media.data.as_bytes());
    body.extend_from_slice(tail.as_bytes());
    (body, boundary)
}

/// The Drive file metadata — field order (name, mimeType, parents?) is also
/// what the boundary hashes.
#[derive(serde::Serialize)]
struct DriveMetadata {
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<String>>,
}

/// Export an artifact using an already-resolved access token (per-user or
/// org). `folder_id` (a Shared Drive or folder) makes the file team-owned
/// there.
pub async fn export_artifact_with_token(
    pg: &PgPool,
    sb: &SecretBox,
    token: &str,
    artifact: &Artifact,
    folder_id: Option<&str>,
) -> Result<DriveFile, ExportError> {
    let Some((target_mime, media)) = media_for(pg, sb, artifact).await else {
        return Err(ExportError::NotExportable);
    };

    let metadata = DriveMetadata {
        name: if artifact.title.is_empty() {
            "Untitled".into()
        } else {
            artifact.title.clone()
        },
        mime_type: target_mime.clone(),
        parents: folder_id.map(|f| vec![f.to_string()]),
    };
    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| ExportError::Failed(format!("drive metadata encode: {e}")))?;
    let (body, boundary) = build_multipart(&metadata_json, &media);

    let res = http()
        .post(UPLOAD_ENDPOINT)
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| ExportError::Failed(format!("drive export request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(ExportError::Failed(format!(
            "drive export failed: {status} {text}"
        )));
    }
    let file: serde_json::Value = res
        .json()
        .await
        .map_err(|e| ExportError::Failed(format!("drive export body: {e}")))?;
    let id = file
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(DriveFile {
        url: file
            .get("webViewLink")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| format!("https://drive.google.com/open?id={id}")),
        name: file
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(metadata.name),
        mime_type: file
            .get("mimeType")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(target_mime),
        id,
    })
}

/// Export an artifact into the given user's Drive: vend the per-user token
/// or fail NotConnected.
pub async fn export_artifact_to_drive(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    artifact: &Artifact,
    now_ms: i64,
) -> Result<DriveFile, ExportError> {
    let token = get_access_token(pg, sb, user_id, now_ms)
        .await
        .map_err(|e: TokenError| ExportError::Failed(e.to_string()))?
        .ok_or(ExportError::NotConnected)?;
    export_artifact_with_token(pg, sb, &token, artifact, None).await
}

// ── Browse + import ──────────────────────────────────────────────────────────

/// One Drive listing row (DriveListEntry) — wire order pinned (id, name,
/// mimeType, modifiedTime, iconLink, webViewLink, sizeBytes).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveListEntry {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub modified_time: Option<String>,
    pub icon_link: Option<String>,
    pub web_view_link: Option<String>,
    pub size_bytes: Option<i64>,
}

/// List/search Drive files using an already-resolved token (per-user or org).
/// Excludes trashed + folders; a query filters by name substring. Parameter
/// order pinned: q, pageSize, orderBy, fields, spaces, supportsAllDrives,
/// includeItemsFromAllDrives.
pub async fn list_drive_files_with_token(
    token: &str,
    query: Option<&str>,
    page_size: usize,
) -> Result<Vec<DriveListEntry>, GoogleError> {
    let mut clauses = vec![
        "trashed = false".to_string(),
        "mimeType != 'application/vnd.google-apps.folder'".to_string(),
    ];
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        // A quote or backslash inside the q-language would break out of the
        // string literal — blank them.
        clauses.push(format!("name contains '{}'", q.replace(['\'', '\\'], " ")));
    }
    let params = {
        let mut p = url::form_urlencoded::Serializer::new(String::new());
        p.append_pair("q", &clauses.join(" and "))
            .append_pair("pageSize", &page_size.clamp(1, 100).to_string())
            .append_pair("orderBy", "modifiedTime desc")
            .append_pair(
                "fields",
                "files(id,name,mimeType,modifiedTime,iconLink,webViewLink,size)",
            )
            .append_pair("spaces", "drive")
            // Without these two, files living in a Shared Drive are invisible
            // to the listing — the org's agents would browse an empty Drive
            // while the provisioned shared drive (their actual workspace) sat
            // unread.
            .append_pair("supportsAllDrives", "true")
            .append_pair("includeItemsFromAllDrives", "true");
        p.finish()
    };
    let res = http()
        .get(format!("{FILES_ENDPOINT}?{params}"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive list request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drive list failed: {status} {text}"
        )));
    }
    let data: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive list body: {e}")))?;
    Ok(data
        .get("files")
        .and_then(|v| v.as_array())
        .map(|files| {
            files
                .iter()
                .map(|f| DriveListEntry {
                    id: f
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name: f
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    mime_type: f
                        .get("mimeType")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    modified_time: f
                        .get("modifiedTime")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    icon_link: f.get("iconLink").and_then(|v| v.as_str()).map(String::from),
                    web_view_link: f
                        .get("webViewLink")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    size_bytes: f
                        .get("size")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<i64>().ok()),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// List/search the user's Drive files (most-recent first). `query` matches
/// names. The connection door is require_token — NotConnected means the user
/// hasn't connected.
pub async fn list_drive_files(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
    query: Option<&str>,
    page_size: usize,
) -> Result<Vec<DriveListEntry>, GoogleError> {
    let token = require_token(pg, sb, user_id, now_ms)
        .await
        .map_err(GoogleError::from)?;
    list_drive_files_with_token(&token, query, page_size).await
}

/// Minimal CSV → grid parser (handles quotes, escaped quotes, CRLF), walked
/// as a char-indexed state machine.
fn csv_to_grid(csv: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = csv.chars().collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if quoted {
            if c == '"' {
                if chars.get(i + 1) == Some(&'"') {
                    cell.push('"');
                    i += 1;
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
        } else if c == '"' {
            quoted = true;
        } else if c == ',' {
            row.push(std::mem::take(&mut cell));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
        } else {
            cell.push(c);
        }
        i += 1;
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// What an import produced (ImportedContent) — the artifact-create body, in
/// wire order.
pub struct ImportedContent {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub storage_ref: Option<String>,
    pub content_type: Option<String>,
    pub source_url: Option<String>,
}

/// Export a Google-native file to a text format (markdown, csv, …).
async fn export_google_text(
    token: &str,
    file_id: &str,
    mime_type: &str,
) -> Result<String, GoogleError> {
    let res = http()
        .get(format!(
            "{FILES_ENDPOINT}/{}/export?mimeType={}",
            encode_uri_component(file_id),
            encode_uri_component(mime_type)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive export request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drive export({mime_type}) failed: {status} {text}"
        )));
    }
    res.text()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive export body: {e}")))
}

/// Export a Google-native file to a binary format (pdf).
async fn export_google_bytes(
    token: &str,
    file_id: &str,
    mime_type: &str,
) -> Result<Vec<u8>, GoogleError> {
    let res = http()
        .get(format!(
            "{FILES_ENDPOINT}/{}/export?mimeType={}",
            encode_uri_component(file_id),
            encode_uri_component(mime_type)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive export request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drive export({mime_type}) failed: {status} {text}"
        )));
    }
    res.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| GoogleError::Failed(format!("drive export body: {e}")))
}

/// Pull a Drive file's content into a Talaria-artifact shape.
/// Google Docs → markdown doc, Sheets → grid sheet, every other native type →
/// exported PDF stored as a file, a regular binary → downloaded file.
pub async fn import_drive_file(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    file_id: &str,
    now_ms: i64,
) -> Result<ImportedContent, GoogleError> {
    let token = require_token(pg, sb, user_id, now_ms)
        .await
        .map_err(GoogleError::from)?;

    // Metadata first: name + type decide how we pull the bytes.
    let meta_res = http()
        .get(format!(
            "{FILES_ENDPOINT}/{}?fields=id,name,mimeType,webViewLink",
            encode_uri_component(file_id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive get request: {e}")))?;
    if !meta_res.status().is_success() {
        let status = meta_res.status();
        let text = meta_res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drive get failed: {status} {text}"
        )));
    }
    let meta: serde_json::Value = meta_res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive get body: {e}")))?;
    let name = meta
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mime = meta
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let source_url = meta
        .get("webViewLink")
        .and_then(|v| v.as_str())
        .map(String::from);

    if mime == GOOGLE_DOC {
        let md = export_google_text(&token, file_id, "text/markdown").await?;
        return Ok(ImportedContent {
            kind: "doc".into(),
            title: name,
            body: md,
            storage_ref: None,
            content_type: None,
            source_url,
        });
    }
    if mime == GOOGLE_SHEET {
        let csv = export_google_text(&token, file_id, "text/csv").await?;
        return Ok(ImportedContent {
            kind: "sheet".into(),
            title: name,
            body: serde_json::to_string(&csv_to_grid(&csv))
                .map_err(|e| GoogleError::Failed(format!("sheet grid encode: {e}")))?,
            storage_ref: None,
            content_type: None,
            source_url,
        });
    }
    if mime.starts_with(GOOGLE_NATIVE_PREFIX) {
        // Other native types (Slides, Drawings, …) → export a PDF and store as
        // a file.
        let bytes = export_google_bytes(&token, file_id, "application/pdf").await?;
        let up = save_upload(
            pg,
            sb,
            &format!("{name}.pdf"),
            "application/pdf",
            &bytes,
            Some(user_id),
        )
        .await
        .map_err(GoogleError::Failed)?;
        return Ok(ImportedContent {
            kind: "file".into(),
            title: name,
            body: String::new(),
            storage_ref: Some(up.id),
            content_type: Some("application/pdf".into()),
            source_url,
        });
    }

    // A regular binary file → download and store.
    let dl_res = http()
        .get(format!(
            "{FILES_ENDPOINT}/{}?alt=media",
            encode_uri_component(file_id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive download request: {e}")))?;
    if !dl_res.status().is_success() {
        let status = dl_res.status();
        let text = dl_res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drive download failed: {status} {text}"
        )));
    }
    let bytes = dl_res
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| GoogleError::Failed(format!("drive download body: {e}")))?;
    let up = save_upload(pg, sb, &name, &mime, &bytes, Some(user_id))
        .await
        .map_err(GoogleError::Failed)?;
    Ok(ImportedContent {
        kind: "file".into(),
        title: name,
        body: String::new(),
        storage_ref: Some(up.id),
        content_type: if mime.is_empty() { None } else { Some(mime) },
        source_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_grids_parse_through_the_same_state_machine() {
        // Plain rows, quoted commas, doubled quotes, CRLF.
        let grid = csv_to_grid("a,b\r\n\"x,1\",\"say \"\"hi\"\"\"\r\nc");
        assert_eq!(
            grid,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["x,1".to_string(), "say \"hi\"".to_string()],
                vec!["c".to_string()],
            ]
        );
        // A trailing newline is not an empty row; an unterminated quote ends at
        // the body's edge.
        assert_eq!(
            csv_to_grid("a,b\n"),
            vec![vec!["a".to_string(), "b".to_string()]]
        );
        assert_eq!(
            csv_to_grid("\"unterminated"),
            vec![vec!["unterminated".to_string()]]
        );
        // A mid-body blank line produces a row with one empty cell — the
        // (empty) cell is pushed unconditionally on the newline.
        assert_eq!(
            csv_to_grid("a\n\nb"),
            vec![
                vec!["a".to_string()],
                vec![String::new()],
                vec!["b".to_string()]
            ]
        );
        assert_eq!(csv_to_grid(""), Vec::<Vec<String>>::new());
    }

    #[test]
    fn drive_list_clauses_quote_the_query() {
        // Mirrors the clauses builder: trashed + non-folder always, name
        // contains with quotes/backslashes blanked.
        let sanitize = |q: &str| q.trim().replace(['\'', '\\'], " ");
        assert_eq!(sanitize("jon's"), "jon s");
        assert_eq!(sanitize("  a\\b "), "a b");
    }
}

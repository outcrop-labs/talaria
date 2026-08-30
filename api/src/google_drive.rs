// Google Drive service — the export half of ui/src/server/google/drive.ts:
// push a Talaria artifact into the connected user's Drive as a native Google
// Doc / Sheet (or an unconverted file). Acts strictly as the connected
// identity via its stored token (per-user or org), so Drive's own sharing
// rules govern what lands where. The browse/import half stays with the
// integrations plane.

use base64::Engine as _;
use sqlx::PgPool;

use crate::artifacts::Artifact;
use crate::gateway::provider::http;
use crate::google_connections::{TokenError, get_access_token};
use crate::secretbox::SecretBox;
use crate::uploads::get_upload;

// supportsAllDrives lets us create into a Shared Drive (team-owned files).
const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&supportsAllDrives=true&fields=id,webViewLink,name,mimeType";

const GOOGLE_DOC: &str = "application/vnd.google-apps.document";
const GOOGLE_SHEET: &str = "application/vnd.google-apps.spreadsheet";

/// drive.ts DriveFile — the created file, in wire order.
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
    /// connections.ts requireToken's GoogleNotConnected throw.
    NotConnected,
    /// mediaFor returned null — the artifact's kind has no Drive mapping.
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
            // String(v) on a JS object is "[object Object]"; these cells come
            // from our own sheet editor and are never containers.
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

/// How each artifact kind maps onto a Drive upload (drive.ts mediaFor).
/// None ⇒ not exportable.
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
            // TS: getUpload(...)'s null (no row or lost blob) ⇒ not exportable.
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
/// itself (TS does the same): deterministic for a given export, collision-safe
/// against any body, and no RNG on the hot path.
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

/// The Drive file metadata — field order is the TS literal's (name, mimeType,
/// parents?), which is also what the boundary hashes.
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

/// Export an artifact into the given user's Drive (drive.ts
/// exportArtifactToDrive): vend the per-user token or fail NotConnected —
/// connections.ts requireToken's caller-friendly throw.
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

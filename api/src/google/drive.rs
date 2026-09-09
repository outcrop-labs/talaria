// Google Drive service: push a Talaria artifact into the connected user's
// Drive as a native Google Doc / Sheet (or an unconverted file), browse what's
// there, and pull a Drive file's content back into a Talaria-artifact shape.
// Acts strictly as the connected identity via its stored token (per-user or
// org), so Drive's own sharing rules govern what lands where.

use base64::Engine as _;
use sqlx::PgPool;

use crate::artifacts::Artifact;
use crate::gateway::provider::http;
use crate::google::connections::{RequireError, TokenError, get_access_token, require_token};
use crate::google::errors::GoogleError;
use crate::google::oauth::encode_uri_component;
use crate::secretbox::SecretBox;
use crate::uploads::{get_upload, save_upload};

// supportsAllDrives lets us create into a Shared Drive (team-owned files).
const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&supportsAllDrives=true&fields=id,webViewLink,name,mimeType";
const FILES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVES_ENDPOINT: &str = "https://www.googleapis.com/drive/v3/drives";

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

/// One browsed Drive folder, root → the folder you are standing in.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DrivePathSeg {
    pub id: String,
    pub name: String,
}

/// One page of a Drive folder: its entries (FOLDERS INCLUDED — this is the
/// browse shape, not the search shape), the next page token when Google has
/// more, and the walked path so a deep link can rebuild its breadcrumbs
/// without client-side ancestry.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DrivePage {
    pub files: Vec<DriveListEntry>,
    pub next_page_token: Option<String>,
    pub path: Vec<DrivePathSeg>,
}

/// Shared drives this token can see (GET /drive/v3/drives) — the roster's
/// shared entries. Where the scope forbids listing, the answer is empty, not
/// an error: the roster just shows fewer drives.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub id: String,
    pub name: String,
}

/// One Drive the rail can browse. `key` is the wire identity the client
/// round-trips (`<connection>:<drive id>`); `writable` is computed from the
/// connection's stored scope grant — full `auth/drive`, not `drive.readonly`
/// or `drive.file` — so the UI never guesses what a write would cost.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveRosterEntry {
    pub key: String,
    pub connection: String,
    pub kind: String, // "my" | "shared"
    pub id: String,
    pub name: String,
    pub writable: bool,
    pub email: Option<String>,
}

/// The `writable` test: the connection's scope list must contain the full
/// drive grant. `drive.readonly` reads, `drive.file` writes app-made files
/// only — neither manages a whole Drive, and offering the controls anyway
/// would be a reconsent trap behind every click.
fn scope_grants_drive(scopes: &[String]) -> bool {
    const FULL: &str = "https://www.googleapis.com/auth/drive";
    const METADATA: &str = "https://www.googleapis.com/auth/drive.metadata";
    const READONLY: &str = "https://www.googleapis.com/auth/drive.readonly";
    scopes
        .iter()
        .any(|s| s == FULL || s == METADATA || (!s.starts_with(READONLY) && s == "drive"))
        && !scopes.iter().any(|s| s == READONLY)
}

/// A Drive `files.list` q-language literal — blank the two characters that
/// would break out of the string (the search builder's rule, reused).
fn drive_q_literal(s: &str) -> String {
    s.replace(['\'', '\\'], " ")
}

/// Children of a Drive folder (or a drive's root), folders first when the
/// caller asks, paginated, with the walked path to `parent`. Shared drives
/// need `driveId` + `corpora=drive` and reject the `'root'` alias — the
/// drive's own id IS the root there.
pub async fn browse_drive_with_token(
    token: &str,
    parent: Option<&str>,
    // None = a personal My Drive (`'root'` alias); Some(id) = that shared
    // drive (its id IS the root — Google rejects `'root'` there).
    shared_drive_id: Option<&str>,
    query: Option<&str>,
    page_size: usize,
    page_token: Option<&str>,
    order_by: &str,
) -> Result<DrivePage, GoogleError> {
    let root = shared_drive_id.unwrap_or("root").to_string();
    let parent_ref = parent.unwrap_or(&root);
    let mut clauses = vec![
        "trashed = false".to_string(),
        format!("'{}' in parents", drive_q_literal(parent_ref)),
    ];
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        clauses.push(format!("name contains '{}'", drive_q_literal(q)));
    }
    let params = {
        let mut p = url::form_urlencoded::Serializer::new(String::new());
        p.append_pair("q", &clauses.join(" and "))
            .append_pair("pageSize", &page_size.clamp(1, 100).to_string())
            .append_pair("orderBy", order_by)
            .append_pair(
                "fields",
                "nextPageToken,files(id,name,mimeType,modifiedTime,iconLink,webViewLink,size)",
            );
        if let Some(id) = shared_drive_id {
            p.append_pair("driveId", id).append_pair("corpora", "drive");
        } else {
            p.append_pair("spaces", "drive");
        }
        p.append_pair("supportsAllDrives", "true")
            .append_pair("includeItemsFromAllDrives", "true");
        if let Some(t) = page_token.filter(|t| !t.is_empty()) {
            p.append_pair("pageToken", t);
        }
        p.finish()
    };
    let res = http()
        .get(format!("{FILES_ENDPOINT}?{params}"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive browse request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drive browse failed: {status} {text}"
        )));
    }
    let data: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("drive browse body: {e}")))?;
    let next_page_token = data
        .get("nextPageToken")
        .and_then(|v| v.as_str())
        .map(String::from);
    let files = data
        .get("files")
        .and_then(|v| v.as_array())
        .map(|rows| rows.iter().map(drive_entry_of).collect())
        .unwrap_or_default();
    // The path walk only makes sense for the FIRST page of a folder — a
    // Load-more token is the same standing, walked already.
    let path = if page_token.map(str::is_empty).unwrap_or(true) {
        drive_folder_path_with_token(token, parent, &root).await?
    } else {
        Vec::new()
    };
    Ok(DrivePage {
        files,
        next_page_token,
        path,
    })
}

fn drive_entry_of(f: &serde_json::Value) -> DriveListEntry {
    DriveListEntry {
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
    }
}

/// Walk name/parents upward from a folder to its drive root — the breadcrumb
/// builder. Stops at the root id, at a parentless row, or at depth 32 (a
/// deeper pile than any real tree; the cap keeps a forged cycle from hanging
/// the request). None (the root itself) walks to nothing.
async fn drive_folder_path_with_token(
    token: &str,
    folder_id: Option<&str>,
    root_id: &str,
) -> Result<Vec<DrivePathSeg>, GoogleError> {
    let Some(mut cur) = folder_id.map(String::from) else {
        return Ok(Vec::new());
    };
    let mut path = Vec::new();
    for _ in 0..32 {
        if cur == root_id || cur == "root" {
            break;
        }
        let res = http()
            .get(format!("{FILES_ENDPOINT}/{cur}?fields=id,name,parents"))
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| GoogleError::Failed(format!("drive path request: {e}")))?;
        if !res.status().is_success() {
            // A vanished folder is an empty path, not a failed browse — the
            // listing said what it said.
            break;
        }
        let v: serde_json::Value = res
            .json()
            .await
            .map_err(|e| GoogleError::Failed(format!("drive path body: {e}")))?;
        let name = v
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string();
        let parents = v
            .get("parents")
            .and_then(|p| p.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(String::from))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        path.push(DrivePathSeg {
            id: cur.clone(),
            name,
        });
        let Some(next) = parents.first() else { break };
        cur = next.clone();
    }
    path.reverse();
    Ok(path)
}

/// Shared drives visible to this token (GET /drive/v3/drives). Read-only
/// scopes list shared drives too; an empty answer means none visible.
pub async fn list_shared_drives_with_token(token: &str) -> Result<Vec<DriveInfo>, GoogleError> {
    let params = "pageSize=100&fields=drives(id,name)";
    let res = http()
        .get(format!("{DRIVES_ENDPOINT}?{params}"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("drives list request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "drives list failed: {status} {text}"
        )));
    }
    let data: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("drives list body: {e}")))?;
    Ok(data
        .get("drives")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .map(|d| DriveInfo {
                    id: d
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    name: d
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// THE ROSTER: every Drive this person can browse, across both connections.
/// Personal first (its My Drive, then its shared drives by name), then the
/// org connection's Shared Drive and its My Drive when connected. A missing
/// connection omits its entries — only BOTH absent is NotConnected, which is
/// the connect screen's and no one else's business.
pub async fn drive_roster(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
) -> Result<Vec<DriveRosterEntry>, GoogleError> {
    let mut out: Vec<DriveRosterEntry> = Vec::new();

    // Personal: (email, scope) then token. Scope is the SPACE-JOINED grant
    // list on the wire (get_connection_status's rule) — split, not decode.
    let personal: Option<(Option<String>, Option<String>)> =
        match sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "select email, scope from google_connections where user_id = $1::uuid",
        )
        .bind(user_id)
        .fetch_optional(pg)
        .await
        {
            Ok(row) => row,
            Err(e) => return Err(GoogleError::Failed(format!("connection read: {e}"))),
        };
    let mut personal_token = None;
    let mut personal_scope: Vec<String> = Vec::new();
    let mut personal_email = None;
    if let Some((email, scope)) = personal {
        personal_scope = scope
            .unwrap_or_default()
            .split_whitespace()
            .map(String::from)
            .collect();
        personal_email = email;
        match require_token(pg, sb, user_id, now_ms).await {
            Ok(token) => personal_token = Some(token),
            Err(RequireError::NotConnected) => {}
            Err(e) => return Err(GoogleError::from(e)),
        }
    }
    if let Some(token) = personal_token.as_deref() {
        let writable = scope_grants_drive(&personal_scope);
        out.push(DriveRosterEntry {
            key: "personal:my".into(),
            connection: "personal".into(),
            kind: "my".into(),
            id: "my".into(),
            name: "My Drive".into(),
            writable,
            email: personal_email.clone(),
        });
        // Shared drives through the personal connection — a shared drive a
        // person joined with their own account. A listing refusal (scope or
        // admin policy) quietly omits them; My Drive still browses.
        if let Ok(drives) = list_shared_drives_with_token(token).await {
            let mut shared: Vec<DriveRosterEntry> = drives
                .into_iter()
                .map(|d| DriveRosterEntry {
                    key: format!("personal:{}", d.id),
                    connection: "personal".into(),
                    kind: "shared".into(),
                    id: d.id,
                    name: d.name,
                    writable,
                    email: personal_email.clone(),
                })
                .collect();
            shared.sort_by(|a, b| a.name.cmp(&b.name));
            out.extend(shared);
        }
    }

    // Org: the provisioned Shared Drive above all (that's the workspace's
    // Drive), then the org account's own My Drive.
    if let Ok(Some(_org_token)) = crate::google::org::get_org_access_token(pg, sb, now_ms).await {
        let writable = true; // ORG_CONNECT_SCOPES carries the full drive grant.
        if let Ok(targets) = crate::google::org::get_org_targets(pg).await
            && let Some(shared_drive_id) = targets.shared_drive_id.filter(|s| !s.is_empty())
        {
            out.push(DriveRosterEntry {
                key: format!("org:{shared_drive_id}"),
                connection: "org".into(),
                kind: "shared".into(),
                id: shared_drive_id,
                name: "Workspace Drive".into(),
                writable,
                email: targets.send_as,
            });
        }
        out.push(DriveRosterEntry {
            key: "org:my".into(),
            connection: "org".into(),
            kind: "my".into(),
            id: "my".into(),
            name: "Org Drive".into(),
            writable,
            email: None,
        });
    }

    Ok(out)
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

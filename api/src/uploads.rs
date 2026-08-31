// Uploads — port of ui/src/server/uploads.ts, whole now that the family
// crosses: the canonical metadata a message stamps when it references files
// (resolveAttachments), the BYTES a transcript rebuild re-reads from a
// textual attachment (getUpload + attachmentTextBlocks), and the write/serve
// plane the routes exercise (saveUpload, the streamed capped form read, and
// serveUpload's inline/download decision). Metadata sits in `uploads`, bytes
// wherever the storage config says (local disk default, or any S3-compatible
// bucket); each row's `path` records where ITS bytes live, so changing the
// mode never strands existing files. The admin storage console's stats and
// detached migrate/sync jobs live here too.

use axum::extract::multipart::Multipart;
use axum::http::header;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use sqlx::PgPool;
use std::sync::LazyLock;
use uuid::Uuid;

use crate::boards::{agent_board_policy_sql, board_visibility_sql};
use crate::secretbox::SecretBox;
use crate::storage::{self, read_blob};

pub const MAX_BYTES: usize = 25 * 1024 * 1024;

/// An upload's canonical metadata (uploads.ts Attachment). `refType` is set
/// only on knowledge/artifact reference chips (refs.rs); upload rows never
/// carry it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
}

/// uploads.ts isImage (`/^image\//.test(mime)`) — the mime that may be
/// referenced as an image block.
pub fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// UPLOADS_DIR — env override, else `.uploads` under the process cwd (the TS
/// server resolves the same way from its own cwd).
pub fn uploads_dir() -> std::path::PathBuf {
    std::env::var("TALARIA_UPLOADS_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join(".uploads"))
}

/// Validate that a set of attachment ids exist (before stamping them on a
/// message) and return their canonical metadata, preserving caller order —
/// including duplicates, which the caller's list asked for twice. Unknown ids
/// are dropped silently: a missing row is not worth failing a whole patch
/// over, and the TS behavior is the same.
pub async fn resolve_attachments(
    pg: &PgPool,
    ids: &[String],
) -> Result<Vec<Attachment>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
        "select id::text, filename, mime, size from uploads where id = any($1::uuid[])",
    )
    .bind(ids.to_vec())
    .fetch_all(pg)
    .await?;
    let by_id: std::collections::HashMap<String, Attachment> = rows
        .into_iter()
        .map(|(id, filename, mime, size)| {
            (
                id.clone(),
                Attachment {
                    id,
                    filename,
                    mime,
                    size,
                },
            )
        })
        .collect();
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

// ── The byte read ──────────────────────────────────────────────────────────

/// One upload's bytes, wherever the row's `path` says they live (getUpload):
/// the row's metadata plus readBlob — disk, the configured bucket, or the
/// replica behind it. A blob that cannot be found is None; the callers that
/// shape transcripts treat a missing file as context that quietly isn't
/// there, never as a failed read. `(bytes, mime, filename)`.
pub async fn get_upload(
    pg: &PgPool,
    sb: &SecretBox,
    id: &str,
) -> Result<Option<(Vec<u8>, String, String)>, sqlx::Error> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("select filename, mime, path from uploads where id = $1::uuid")
            .bind(id)
            .fetch_optional(pg)
            .await?;
    let Some((filename, mime, path)) = row else {
        return Ok(None);
    };
    // The blob read is not a database concern: a lost blob (file deleted,
    // bucket rehomed) is a missing upload, not a failed query.
    Ok(read_blob(pg, sb, &path)
        .await
        .map(|bytes| (bytes, mime, filename)))
}

/// uploads.ts TEXT_MIME — the mimes whose bytes become prompt context.
static TEXT_MIME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^text/|^application/(json|xml|javascript|x-yaml|yaml|csv|toml|sql|markdown)|(\+json|\+xml)$",
    )
    .unwrap()
});

/// How much of a file's text rides in one block (FILE_CLIP): a transcript is
/// context, not an archive.
const FILE_CLIP: usize = 6_000;

fn file_candidates(attachments: &serde_json::Value) -> Vec<&serde_json::Value> {
    let Some(arr) = attachments.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|a| {
            a.as_object().is_some_and(|o| !o.contains_key("refType"))
                && a.get("mime")
                    .and_then(|m| m.as_str())
                    .is_some_and(|m| TEXT_MIME.is_match(m))
        })
        .collect()
}

/// uploads.ts attachmentAsDataUrl: one image upload as a `data:<mime>;base64`
/// URL — the block a vision model sees. A missing row or a lost blob is None
/// (the caller's `.catch(() => null)` skip), never an error.
pub async fn attachment_as_data_url(pg: &PgPool, sb: &SecretBox, id: &str) -> Option<String> {
    let (bytes, mime, _filename) = get_upload(pg, sb, id).await.ok()??;
    use base64::Engine as _;
    let mut url = format!("data:{mime};base64,");
    base64::engine::general_purpose::STANDARD.encode_string(bytes, &mut url);
    Some(url)
}

/// The prompt block a message's textual file uploads contribute
/// (attachmentTextBlocks): up to `max_files` text-mime attachments, each
/// re-read from storage and clipped. Ref chips are NOT file uploads (they
/// carry their content inline — ref_blocks); a non-array or a row whose blob
/// is gone contributes nothing, exactly TS's `.catch(() => null)` skip.
pub async fn attachment_text_blocks(
    pg: &PgPool,
    sb: &SecretBox,
    attachments: &serde_json::Value,
    max_files: usize,
) -> String {
    let files = file_candidates(attachments);
    let mut blocks: Vec<String> = Vec::new();
    for f in files.iter().take(max_files) {
        let id = f.get("id").and_then(|i| i.as_str()).unwrap_or("");
        let Ok(Some((bytes, _mime, filename))) = get_upload(pg, sb, id).await else {
            continue;
        };
        // Buffer.toString('utf8') replaces invalid sequences with U+FFFD —
        // from_utf8_lossy is the same replacement, not a failure.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let clipped = if crate::body::utf16_len(&text) > FILE_CLIP {
            format!(
                "{}\n[clipped]",
                crate::body::truncate_utf16(&text, FILE_CLIP)
            )
        } else {
            text
        };
        blocks.push(format!(
            "\n\n--- Attached file: \"{filename}\" ---\n{clipped}"
        ));
    }
    blocks.join("")
}

// ── Serving: the inline/download decision ──────────────────────────────────

/// The ONLY types the app will ever render INLINE. `image/svg+xml` and every
/// `text/*` are deliberately absent: both execute as HTML in a browser, an
/// upload's MIME is whatever the uploader's client declared, and an inline
/// response is SAME-ORIGIN — script in it runs with the viewer's session.
/// Raster images are inert data; PDF renders inside the browser's own viewer
/// sandbox.
const INLINE_MIME: [&str; 6] = [
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/avif",
    "application/pdf",
];

/// The upload's bytes as served (uploads.ts serveUpload) — the single place
/// the inline/download decision is made, so no route can widen it on its own.
/// Downloads additionally carry a sandbox CSP: even a type mislisted above
/// stays inert when navigated to directly, on top of the attachment
/// disposition.
pub fn serve_upload(bytes: Vec<u8>, mime: &str, filename: &str, cache: &str) -> Response {
    let inline = INLINE_MIME.contains(&mime);
    // CR/LF would smuggle extra header lines; a quote would break out of the
    // quoted-string. The filename is display metadata, never a path.
    let name: String = filename
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n' | '"' | '\\'))
        .collect();
    let mut res = Response::new(axum::body::Body::from(bytes));
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(mime).expect("mime is header-safe"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!(
            "{}; filename=\"{name}\"",
            if inline { "inline" } else { "attachment" }
        ))
        .expect("disposition is header-safe"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_str(cache).expect("cache is header-safe"),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        header::HeaderValue::from_static("nosniff"),
    );
    if !inline {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static("default-src 'none'; sandbox"),
        );
    }
    res
}

// ── Reading the body without trusting its size (#266) ─────────────────────

/// Multipart envelope beyond the file bytes: boundaries, field names, a
/// margin for incidental fields. The stream cap is file + envelope — anything
/// bigger is refused before the form has a reason to finish parsing.
const FORM_ENVELOPE_BYTES: usize = 64 * 1024;

/// readUploadForm's outcome. TS reads the whole form then asks for the `file`
/// entry; the first `file`-named part decides, exactly like FormData.get — if
/// it carries no filename it is a plain string field, never skipped over for a
/// later one. Both failure reasons map to the same 400 (`no file`) at the
/// route, so Malformed and NoFile stay distinct only for the log line.
pub enum FormRead {
    /// (filename, mime, bytes) of the first `file` part.
    File(String, String, Vec<u8>),
    TooLarge,
    Malformed,
    /// No `file` part, or the first one is a plain text field.
    NoFile,
}

/// Content-length is checked first — the free win, refusing a sized attack at
/// connect time — but a streamed multipart body (what every browser FormData
/// POST is) carries no usable content-length, so the read itself is capped:
/// the running total crosses the cap and the answer is TooLarge having read
/// `cap` bytes instead of whatever the client felt like sending.
pub async fn read_upload_form(headers: &HeaderMap, mut multipart: Multipart) -> FormRead {
    let cap = MAX_BYTES + FORM_ENVELOPE_BYTES;
    if let Some(len) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        && len > cap
    {
        return FormRead::TooLarge;
    }
    let mut total: usize = 0;
    let mut file: Option<(String, String, Vec<u8>)> = None;
    let mut saw_file_key = false;
    let mut no_file = false;
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        let is_file_part = field.file_name().is_some();
        let mut data = Vec::new();
        let mut malformed = false;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    total += chunk.len();
                    if total > cap {
                        return FormRead::TooLarge;
                    }
                    data.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => {
                    malformed = true;
                    break;
                }
            }
        }
        if name != "file" || saw_file_key {
            continue;
        }
        saw_file_key = true;
        if malformed {
            return FormRead::Malformed;
        }
        if !is_file_part {
            no_file = true;
            continue;
        }
        let mime = field.content_type().map(str::to_string).unwrap_or_default();
        let filename = field.file_name().unwrap_or("").to_string();
        file = Some((filename, mime, data));
    }
    if let Some((filename, mime, bytes)) = file {
        return FormRead::File(filename, mime, bytes);
    }
    if no_file {
        return FormRead::NoFile;
    }
    FormRead::NoFile
}

// ── Write ────────────────────────────────────────────────────────────────────

/// extname → keep only `.` and alphanumerics, ≤12 chars — the safe tail of
/// the original name, same shape TS keeps. A leading dot is the dotfile
/// convention, not an extension (Node's extname agrees).
fn safe_ext(filename: &str) -> String {
    let Some(dot) = filename.rfind('.') else {
        return String::new();
    };
    if dot == 0 {
        return String::new();
    }
    filename[dot..]
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .take(12)
        .collect()
}

/// saveUpload — id first (the key and the disk name derive from it), bytes to
/// the active target or disk, a detached replica mirror that must never fail
/// the upload, then the row. Returns the FULL input filename/mime (the row
/// stores the sliced forms; the wire answers what the caller sent).
pub async fn save_upload(
    pg: &PgPool,
    sb: &SecretBox,
    filename: &str,
    mime: &str,
    bytes: &[u8],
    user_id: Option<&str>,
) -> Result<Attachment, String> {
    if bytes.len() > MAX_BYTES {
        return Err("file too large (max 25 MB)".into());
    }
    let id = Uuid::new_v4().to_string();
    let ext = safe_ext(filename);
    let cfg = storage::get_storage_config(pg, sb).await;
    let active = storage::active_target(&cfg).await?;
    let path = match active {
        Some((target, internal)) => {
            let key = format!("{}uploads/{id}{ext}", target.prefix);
            storage::s3_put(&target, &key, bytes, mime).await?;
            format!(
                "{}://{}/{key}",
                if internal { "s3+internal" } else { "s3" },
                target.bucket
            )
        }
        None => {
            let dir = uploads_dir();
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| e.to_string())?;
            let path = dir.join(format!("{id}{ext}"));
            tokio::fs::write(&path, bytes)
                .await
                .map_err(|e| e.to_string())?;
            path.to_string_lossy().into_owned()
        }
    };
    // Mirror to the replica off the request path — a replica outage must
    // never fail an upload. The full sync catches anything missed here.
    if let Some(replica) = storage::replica_target(&cfg) {
        let key = format!("{}uploads/{id}{ext}", replica.prefix);
        let bytes = bytes.to_vec();
        let mime = mime.to_string();
        tokio::spawn(async move {
            if let Err(e) = storage::s3_put(&replica, &key, &bytes, &mime).await {
                tracing::error!("[storage] replica write failed: {e}");
            }
        });
    }
    let _ = sqlx::query(
        "insert into uploads (id, filename, mime, size, path, uploaded_by) \
         values ($1::uuid, $2, $3, $4, $5, $6::uuid)",
    )
    .bind(&id)
    .bind(crate::body::truncate_utf16(filename, 300))
    .bind(crate::body::truncate_utf16(mime, 120))
    .bind(bytes.len() as i64)
    .bind(&path)
    .bind(user_id)
    .execute(pg)
    .await;
    Ok(Attachment {
        id,
        filename: filename.to_string(),
        mime: mime.to_string(),
        size: bytes.len() as i64,
    })
}

// ── Access ──────────────────────────────────────────────────────────────────

/// Who is asking to fetch an upload's bytes.
pub enum UploadViewer<'a> {
    Agent {
        model: &'a str,
    },
    Human {
        user_id: &'a str,
        who: Option<&'a str>,
        is_admin: bool,
    },
}

/// canAccessUpload — owner and admins always; anyone else only when the upload
/// is REACHABLE through a container they can read (a conversation they're in,
/// a channel they're a member of, a ticket on a board they belong to, or an
/// artifact they can read). Agents mirror that through their own access model.
/// Fail closed.
pub async fn can_access_upload(pg: &PgPool, upload_id: &str, viewer: UploadViewer<'_>) -> bool {
    // TS: JSON.stringify([{ id: uploadId }]) — the containment probe.
    let r#ref = format!(r#"[{{"id":"{upload_id}"}}]"#);

    if let UploadViewer::Agent { model } = viewer {
        // Ticket on a board that allows the agent.
        // AssertSqlSafe: the interpolated fragment is boards.rs's
        // agent_board_policy_sql.
        let task_sql = format!(
            "select 1 as ok from tasks t join boards b on b.id = t.board_id \
             where t.attachments @> $1::jsonb and {} limit 1",
            agent_board_policy_sql("$2")
        );
        let task: Option<(i32,)> = sqlx::query_as(sqlx::AssertSqlSafe(task_sql.as_str()))
            .bind(&r#ref)
            .bind(model)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
        if task.is_some() {
            return true;
        }
        // Message in a channel the agent belongs to.
        let ch: Option<(i32,)> = sqlx::query_as(
            "select 1 as ok from channel_messages cm \
             where cm.attachments @> $1::jsonb \
             and exists(select 1 from channel_agents ca where ca.channel_id = cm.channel_id and ca.agent_model = $2) \
             limit 1",
        )
        .bind(&r#ref)
        .bind(model)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
        if ch.is_some() {
            return true;
        }
        // A personal assistant reads through its owner's conversations.
        let conv: Option<(i32,)> = sqlx::query_as(
            "select 1 as ok from messages m \
             join conversations c on c.id = m.conversation_id \
             join agent_defs d on d.owner_user_id = c.user_id and d.model = $2 \
             where m.attachments @> $1::jsonb limit 1",
        )
        .bind(&r#ref)
        .bind(model)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
        return conv.is_some();
    }

    let UploadViewer::Human {
        user_id,
        who,
        is_admin,
    } = viewer
    else {
        return false;
    };
    if is_admin {
        return true;
    }
    let own: Option<(i32,)> = sqlx::query_as(
        "select 1 as ok from uploads where id = $1::uuid and uploaded_by = $2::uuid limit 1",
    )
    .bind(upload_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    if own.is_some() {
        return true;
    }
    // An image embedded in a KB doc: readable by whoever can read the doc.
    let docs: Vec<(String,)> =
        sqlx::query_as("select id::text from kb_docs where body like $1 limit 5")
            .bind(format!("%{upload_id}%"))
            .fetch_all(pg)
            .await
            .unwrap_or_default();
    for (doc_id,) in docs {
        let Ok(Some(doc)) = crate::kb::get_doc(pg, &doc_id).await else {
            continue;
        };
        if let Ok(effective) = crate::kb::effective_doc_perms(pg, &doc).await
            && crate::kb_perms::can_read(&effective.perms, Some(user_id), who, &effective.grants)
        {
            return true;
        }
    }

    // AssertSqlSafe: the interpolated fragment is boards.rs's
    // board_visibility_sql.
    let reach_sql = format!(
        "select 1 as ok where \
         exists(\
           select 1 from messages m join conversations c on c.id = m.conversation_id \
           where m.attachments @> $1::jsonb \
             and (c.user_id = $2::uuid \
               or (c.kind = 'plan' and exists(select 1 from conversation_members cm where cm.conversation_id = c.id and cm.user_id = $2::uuid)))\
         ) \
         or exists(\
           select 1 from channel_messages cm \
           where cm.attachments @> $1::jsonb \
             and exists(select 1 from channel_members x where x.channel_id = cm.channel_id and x.user_id = $2::uuid)\
         ) \
         or exists(\
           select 1 from tasks t join boards b on b.id = t.board_id \
           where t.attachments @> $1::jsonb and {}\
         ) limit 1",
        board_visibility_sql("$2", "$2", false)
    );
    let reach: Option<(i32,)> = sqlx::query_as(sqlx::AssertSqlSafe(reach_sql.as_str()))
        .bind(&r#ref)
        .bind(user_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
    if reach.is_some() {
        return true;
    }
    // Artifact whose file IS this upload — visible per the artifact's own ACL.
    let arts: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select id::text, owner_user_id::text, created_by::text, visibility \
         from artifacts where storage_ref = $1::uuid limit 3",
    )
    .bind(upload_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for (id, owner_user_id, created_by, visibility) in arts {
        if visibility != "private" {
            return true;
        }
        if owner_user_id.as_deref() == Some(user_id) {
            return true;
        }
        if let Some(who) = who
            && created_by.as_deref() == Some(who)
        {
            return true;
        }
        let grant: Option<(i32,)> = sqlx::query_as(
            "select 1 as ok from kb_editors where item_type = 'artifact' and item_id = $1::uuid \
             and principal_type = 'user' and principal_id = $2::uuid limit 1",
        )
        .bind(&id)
        .bind(user_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
        if grant.is_some() {
            return true;
        }
    }
    false
}

/// The `not found` both upload routes answer — house envelope, 404.
pub fn upload_not_found() -> Response {
    crate::error::house_error(StatusCode::NOT_FOUND, "not found")
}

// ── The admin storage console (uploads.ts uploadStats + migrate/sync) ────────

/// Where every blob lives, by path prefix (uploadStats): `s3://…` external,
/// `s3+internal://…` the bundled bucket, anything else local disk — plus the
/// local-disk byte total.
pub async fn upload_stats(pg: &PgPool) -> serde_json::Value {
    let row: Result<(i64, i64, i64, i64), _> = sqlx::query_as(
        "select count(*) filter (where path like 's3://%'), \
                count(*) filter (where path like 's3+internal://%'), \
                count(*) filter (where path not like 's3%'), \
                coalesce(sum(size) filter (where path not like 's3%'), 0)::bigint \
         from uploads",
    )
    .fetch_one(pg)
    .await;
    match row {
        Ok((s3, internal, local, local_bytes)) => serde_json::json!({
            "local": local,
            "s3": s3,
            "internal": internal,
            "localBytes": local_bytes,
        }),
        Err(_) => serde_json::json!({ "local": 0, "s3": 0, "internal": 0, "localBytes": 0 }),
    }
}

/// One detached job's progress, as the console polls it (MigrateStatus).
/// Hand-built so `error`/`finishedAt` are OMITTED until they exist, exactly
/// like TS's optional fields.
#[derive(Clone)]
struct JobStatus {
    running: bool,
    moved: i64,
    failed: i64,
    total: i64,
    error: Option<String>,
    finished_ms: Option<i64>,
}

impl JobStatus {
    fn to_json(&self) -> serde_json::Value {
        let mut f = serde_json::Map::new();
        f.insert("running".into(), self.running.into());
        f.insert("moved".into(), self.moved.into());
        f.insert("failed".into(), self.failed.into());
        f.insert("total".into(), self.total.into());
        if let Some(e) = &self.error {
            f.insert("error".into(), e.clone().into());
        }
        if let Some(ms) = self.finished_ms {
            f.insert(
                "finishedAt".into(),
                crate::agent_auth::epoch_ms_to_iso(ms).into(),
            );
        }
        serde_json::Value::Object(f)
    }

    async fn save(&self, pg: &PgPool, key: &str) {
        let _ = crate::gateway::settings::set_setting(pg, key, &self.to_json()).await;
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

const MIGRATE_KEY: &str = "storage_migrate_status";
const SYNC_KEY: &str = "storage_sync_status";

/// The last recorded migrate status, or null when it never ran.
pub async fn migrate_status(pg: &PgPool) -> serde_json::Value {
    crate::gateway::settings::get_setting(pg, MIGRATE_KEY, serde_json::Value::Null).await
}

/// The last recorded sync status, or null when it never ran.
pub async fn sync_status(pg: &PgPool) -> serde_json::Value {
    crate::gateway::settings::get_setting(pg, SYNC_KEY, serde_json::Value::Null).await
}

/// Move every local-disk blob into the active bucket (internal or external).
/// Runs DETACHED; the caller polls migrateStatus(). Local files are left in
/// place (the row's path is the source of truth) — uploads-dir cleanup is the
/// operator's call. A job already running is returned, not double-started.
pub async fn migrate_uploads_to_s3(
    pg: &PgPool,
    sb: &SecretBox,
) -> Result<serde_json::Value, String> {
    let prior = migrate_status(pg).await;
    if prior.get("running") == Some(&serde_json::Value::Bool(true)) {
        return Ok(prior);
    }
    let cfg = storage::get_storage_config(pg, sb).await;
    let Some((target, internal)) = storage::active_target(&cfg).await? else {
        return Err("object storage is not configured".into());
    };
    let scheme = if internal { "s3+internal" } else { "s3" };
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "select id::text, path, mime from uploads where path not like 's3%' order by created_at asc",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let status = JobStatus {
        running: true,
        moved: 0,
        failed: 0,
        total: rows.len() as i64,
        error: None,
        finished_ms: None,
    };
    status.save(pg, MIGRATE_KEY).await;
    let initial = status.to_json();
    let pg = pg.clone();
    let mut job = status.clone();
    tokio::spawn(async move {
        for (id, path, mime) in rows {
            let step = async {
                let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
                let key = format!("{}uploads/{}{}", target.prefix, id, safe_ext(&path));
                storage::s3_put(&target, &key, &bytes, &mime).await?;
                let recorded = format!("{scheme}://{}/{}", target.bucket, key);
                sqlx::query("update uploads set path = $1 where id = $2::uuid")
                    .bind(&recorded)
                    .bind(&id)
                    .execute(&pg)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            };
            match step.await {
                Ok(()) => job.moved += 1,
                Err(_) => job.failed += 1,
            }
            if (job.moved + job.failed) % 10 == 0 {
                job.save(&pg, MIGRATE_KEY).await;
            }
        }
        job.running = false;
        job.finished_ms = Some(JobStatus::now_ms());
        job.save(&pg, MIGRATE_KEY).await;
    });
    Ok(initial)
}

/// blobBasename — the last path segment, for a disk path or an s3:// URI
/// alike (`path.slice(path.lastIndexOf('/') + 1)`).
fn blob_basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Copy EVERY blob (disk, internal, external — wherever each lives) into the
/// replica bucket, keyed canonically so per-upload mirror writes and this
/// full sync land on the same objects. Runs DETACHED; poll syncStatus().
pub async fn sync_uploads_to_replica(
    pg: &PgPool,
    sb: &SecretBox,
) -> Result<serde_json::Value, String> {
    let prior = sync_status(pg).await;
    if prior.get("running") == Some(&serde_json::Value::Bool(true)) {
        return Ok(prior);
    }
    let cfg = storage::get_storage_config(pg, sb).await;
    let Some(replica) = storage::replica_target(&cfg) else {
        return Err("replica is not configured (enable it and fill in every field)".into());
    };
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("select id::text, path, mime from uploads order by created_at asc")
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
    let status = JobStatus {
        running: true,
        moved: 0,
        failed: 0,
        total: rows.len() as i64,
        error: None,
        finished_ms: None,
    };
    status.save(pg, SYNC_KEY).await;
    let initial = status.to_json();
    let pg = pg.clone();
    let sb = sb.clone();
    let mut job = status.clone();
    tokio::spawn(async move {
        for (_id, path, mime) in rows {
            let step = async {
                let bytes = storage::read_blob(&pg, &sb, &path)
                    .await
                    .ok_or_else(|| "unreadable".to_string())?;
                let key = format!("{}uploads/{}", replica.prefix, blob_basename(&path));
                storage::s3_put(&replica, &key, &bytes, &mime).await?;
                Ok::<(), String>(())
            };
            match step.await {
                Ok(()) => job.moved += 1,
                Err(_) => job.failed += 1,
            }
            if (job.moved + job.failed) % 10 == 0 {
                job.save(&pg, SYNC_KEY).await;
            }
        }
        job.running = false;
        job.finished_ms = Some(JobStatus::now_ms());
        job.save(&pg, SYNC_KEY).await;
    });
    Ok(initial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_mime_matches_the_js_regex() {
        for m in [
            "text/plain",
            "text/csv",
            "application/json",
            "application/xml",
            "application/javascript",
            "application/x-yaml",
            "application/yaml",
            "application/csv",
            "application/toml",
            "application/sql",
            "application/markdown",
            "application/vnd.api+json",
            "application/ld+json",
            "image/svg+xml",
            "application/xhtml+xml",
            // The ^-anchored alternatives have no end anchor in the JS
            // pattern, so a PREFIX matches: "jsonl" starts with "json".
            "application/jsonl",
        ] {
            assert!(TEXT_MIME.is_match(m), "should match {m}");
        }
        for m in [
            "image/png",
            "application/pdf",
            "application/octet-stream",
            "textish/plain",
            "application/fooxml",
        ] {
            assert!(!TEXT_MIME.is_match(m), "should not match {m}");
        }
    }

    /// The FILTER is the whole testable surface without storage: which array
    /// members are file candidates at all. The bytes come from read_blob,
    /// which storage's own suite covers.
    #[test]
    fn file_candidates_exclude_refs_nonobjects_and_nontext() {
        let a = json!([
            { "id": "1", "mime": "text/plain", "filename": "a.txt" },
            { "id": "2", "mime": "image/png" },
            { "refType": "kb-doc", "mime": "text/plain", "content": "x" },
            { "mime": "text/plain" },
            "not an object",
            { "id": "3", "mime": "application/json" }
        ]);
        let c = file_candidates(&a);
        // Three candidates: the id-less text entry DOES pass the filter — in
        // TS its read then misses (getUpload(undefined) → catch → skip), so
        // the filter itself must keep it.
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].get("id").unwrap(), "1");
        assert!(c[1].get("id").is_none());
        assert_eq!(c[2].get("id").unwrap(), "3");
        // A non-array answers no candidates at all (refBlocks' shape rule).
        assert!(file_candidates(&json!(null)).is_empty());
        assert!(file_candidates(&json!({})).is_empty());
    }

    /// The inline/download allowlist, and the header hygiene on both sides.
    #[test]
    fn serve_upload_decides_inline_by_allowlist_only() {
        let res = serve_upload(
            vec![1, 2, 3],
            "image/png",
            "a\"b\nc.png",
            "private, max-age=1",
        );
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/png"
        );
        assert_eq!(
            res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            "inline; filename=\"abc.png\""
        );
        assert!(res.headers().get(header::CONTENT_SECURITY_POLICY).is_none());

        // Everything not on the list downloads, sandboxed and nosniffed.
        let res = serve_upload(vec![1], "image/svg+xml", "x.svg", "no-store");
        assert_eq!(
            res.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"x.svg\""
        );
        assert_eq!(
            res.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "default-src 'none'; sandbox"
        );
        assert_eq!(
            res.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
    }

    #[test]
    fn safe_ext_keeps_the_inert_tail() {
        assert_eq!(safe_ext("a.tar.gz"), ".gz");
        assert_eq!(safe_ext("noext"), "");
        assert_eq!(safe_ext("x./weird\\name.png"), ".png");
        // A dotfile's dot is the name, not an extension (path.extname agrees).
        assert_eq!(safe_ext(".hidden"), "");
    }
}

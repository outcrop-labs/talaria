// Agent Google Docs. Reads are free. Creating a new doc is immediate when it
// lands in the account's own Drive or in a folder this agent created — it
// destroys nothing and shows nobody anything. Creating one inside any other
// folder queues a `doc_create`: a file placed in a shared folder inherits
// that folder's sharing, which is a disclosure a person approves. Updating
// a doc the agent did not create queues a `doc_update` for a human. Updating
// one this agent created is immediate — until a person edits it, after which
// a full-body replace queues too (an append stays immediate). "A person
// edited it" is read two ways before a replace: Drive's `lastModifyingUser.me`
// (someone other than the token holder), and Drive's `modifiedTime` against
// the stamp Talaria takes on every agent write (the token holder themselves —
// the owner, from their own account, which the `me` flag cannot tell apart).
//
// GET  /docs/{id}           read markdown
// POST /docs                create
// POST /docs/{id}           update or append
// GET  /files?q=            fullText search
// GET  /files/{id}          non-native bytes (utf-8, else base64)
// POST /files/{id}/import   pull into a Talaria artifact

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_agent_auth::{AgentSubject, refuse_legacy, require_agent};
use talaria_api_facades::google::agent::{resolve_agent_google, resolve_agent_principal};
use talaria_api_facades::google::docs::{
    create_doc_with_token, get_doc_with_token, last_modifier_with_token, modified_after_agent,
    read_file_media_with_token, search_files_fulltext, update_doc_with_token,
};
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::oauth::query_pairs;
use talaria_api_facades::google::pending_actions::{
    QueueAction, agent_created, agent_created_record, forget_agent_created, queue_action,
    record_agent_created, stamp_agent_write,
};
use talaria_body::{optional_boolean_member, optional_max_string_member, parse, string_member};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

const NOT_CONNECTED: &str =
    "No Google account is connected for this agent (its owner, or the org account).";

fn not_connected() -> Response {
    house_error_msg(StatusCode::CONFLICT, "not_connected", NOT_CONNECTED)
}

/// Whether a caller-chosen parent needs a person: absent or empty means the
/// account's own Drive (nothing shared); a folder this agent created is its
/// own scratch space; anything else is somebody's folder.
pub(crate) async fn parent_needs_approval(
    pg: &sqlx::PgPool,
    agent_model: &str,
    parent: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let Some(parent) = parent.filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    Ok(!agent_created(pg, agent_model, parent).await?)
}

pub async fn get_doc(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Docs") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    match get_doc_with_token(&google.token, &id).await {
        Ok(doc) => Ok(Json(json!({
            "id": doc.id,
            "title": doc.title,
            "url": doc.url,
            "mimeType": doc.mime_type,
            "markdown": doc.markdown,
        }))
        .into_response()),
        Err(e) => Ok(google_fail(e, "Docs")),
    }
}

pub async fn post_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Docs") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let title = match optional_max_string_member(obj, "title", 500) {
        Ok(Some(s)) if !s.is_empty() => s,
        Ok(_) => "Untitled".to_string(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let markdown = match optional_max_string_member(obj, "body", 500_000) {
        Ok(Some(s)) => s,
        Ok(None) => String::new(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let folder_id = match optional_max_string_member(obj, "folderId", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| internal("[integrations/google/agent/docs] principal read failed", e))?
    {
        return Ok(denied);
    }
    let folder_id = folder_id.filter(|s| !s.is_empty());
    let needs_approval =
        match parent_needs_approval(&state.pg, &agent_model, folder_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(internal(
                    "[integrations/google/agent/docs] parent ownership read failed",
                    e,
                ));
            }
        };
    if needs_approval {
        // Somebody's folder: the doc inherits its sharing the moment it
        // exists, so a person says yes first — the same card an email gets.
        let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(internal(
                    "[integrations/google/agent/docs] principal read failed",
                    e,
                ));
            }
        };
        let mut payload = serde_json::Map::new();
        payload.insert("title".into(), json!(title));
        payload.insert("markdown".into(), json!(markdown));
        payload.insert("folderId".into(), json!(folder_id));
        let summary = format!(
            "Create Google Doc \"{title}\" in folder {}",
            folder_id.as_deref().unwrap_or("")
        );
        let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
        let queued = match queue_action(
            &state.pg,
            realtime,
            &QueueAction {
                kind: "doc_create",
                summary: &summary,
                payload: &Value::Object(payload),
                agent_model: &agent_model,
                owner_user_id: principal.owner_user_id.as_deref(),
                is_org: principal.is_org,
                principal_kind: principal.kind.as_str(),
            },
        )
        .await
        {
            Ok(q) => q,
            Err(e) => return Ok(internal("[integrations/google/agent/docs] queue failed", e)),
        };
        return Ok(super::integrations_google_agent_queue::answer_queued(
            &state.pg,
            queued,
            "A doc in this folder is already waiting for approval — nothing new queued.",
            "Queued — that folder is not this agent's, so the owner approves before the doc is created there.",
            "Queued — that folder is not this agent's, so an admin approves before the doc is created there.",
        )
        .await);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    let doc =
        match create_doc_with_token(&google.token, &title, &markdown, folder_id.as_deref()).await {
            Ok(d) => d,
            Err(e) => return Ok(google_fail(e, "Docs")),
        };
    if let Err(e) = record_agent_created(&state.pg, &agent_model, &doc.id).await {
        return Ok(internal(
            "[integrations/google/agent/docs] created-doc record failed",
            e,
        ));
    }
    Ok(Json(json!({
        "id": doc.id,
        "url": doc.url,
        "name": doc.name,
        "mimeType": doc.mime_type,
        "createdByAgent": true,
        "message": "Created. This agent can edit it without another approval until a person edits it; a human-owned doc still queues.",
    }))
    .into_response())
}

pub async fn post_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Docs") {
        return Ok(denied);
    }
    let agent_model = caller.model.clone();
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let markdown = match string_member(obj, "body", 0, 500_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let title = match optional_max_string_member(obj, "title", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let append = match optional_boolean_member(obj, "append") {
        Ok(v) => v.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(denied) =
        super::integrations_google_agent_queue::refuse_unresolved_write(&state.pg, &agent_model)
            .await
            .map_err(|e| internal("[integrations/google/agent/docs] principal read failed", e))?
    {
        return Ok(denied);
    }
    let principal = match resolve_agent_principal(&state.pg, &agent_model).await {
        Ok(p) => p,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/docs] principal read failed",
                e,
            ));
        }
    };
    let created = match agent_created_record(&state.pg, &agent_model, &id).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/docs] ownership read failed",
                e,
            ));
        }
    };
    let mut owns = created.is_some();
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    // Ownership expires when a person takes the doc over. Drive says who
    // last changed the file and whether that was the token holder; an edit
    // by anyone else ends the agent's standing to replace the body without a
    // card, and the record is dropped so the next edit queues too. The owner
    // editing their own assistant's doc IS the token holder, which Drive
    // cannot tell from the agent — so Drive's `modifiedTime` is also held
    // against the stamp Talaria took when the agent last wrote the doc: a
    // revision later than the agent's own write was a person's, whoever
    // Drive says made it. An append adds and destroys nothing, so it stays
    // immediate.
    if owns && !append {
        let last_agent_write_ms = created.as_ref().and_then(|c| c.last_agent_write_ms);
        match last_modifier_with_token(&google.token, &id).await {
            Ok(modifier)
                if modifier.by_token_holder == Some(false)
                    || modified_after_agent(
                        modifier.modified_time.as_deref(),
                        last_agent_write_ms,
                    ) =>
            {
                if let Err(e) = forget_agent_created(&state.pg, &agent_model, &id).await {
                    return Ok(internal(
                        "[integrations/google/agent/docs] ownership expiry failed",
                        e,
                    ));
                }
                owns = false;
            }
            Ok(_) => {}
            Err(e) => return Ok(google_fail(e, "Docs")),
        }
    }
    let next = if append {
        match get_doc_with_token(&google.token, &id).await {
            Ok(current) => format!("{}\n{}", current.markdown, markdown),
            Err(e) => return Ok(google_fail(e, "Docs")),
        }
    } else {
        markdown
    };
    if owns {
        let doc = match update_doc_with_token(&google.token, &id, &next, title.as_deref()).await {
            Ok(doc) => doc,
            Err(e) => return Ok(google_fail(e, "Docs")),
        };
        // Stamped after Drive answered, so the stamp is never earlier than
        // the revision it covers; the next replace reads it.
        if let Err(e) = stamp_agent_write(&state.pg, &agent_model, &id).await {
            return Ok(internal(
                "[integrations/google/agent/docs] agent write stamp failed",
                e,
            ));
        }
        return Ok(Json(json!({
            "id": doc.id,
            "url": doc.url,
            "name": doc.name,
            "mimeType": doc.mime_type,
            "createdByAgent": true,
            "message": "Updated. This agent created the doc, so the edit was not queued.",
        }))
        .into_response());
    }
    // One pending edit per file. A second draft while the first waits returns
    // that row instead of flooding the queue.
    let existing: Option<(String,)> = match sqlx::query_as(
        "select id::text from google_pending_actions \
         where kind = 'doc_update' and status = 'pending' and agent_model = $1 \
           and payload->>'fileId' = $2 \
         order by created_at desc limit 1",
    )
    .bind(&agent_model)
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/docs] dedupe read failed",
                e,
            ));
        }
    };
    if let Some((pending_id,)) = existing {
        return Ok(Json(json!({
            "pending": { "id": pending_id, "status": "pending", "kind": "doc_update" },
            "message": "An edit to this doc is already waiting for approval — nothing new queued.",
        }))
        .into_response());
    }
    let mut payload = serde_json::Map::new();
    payload.insert("fileId".into(), json!(id));
    payload.insert("markdown".into(), json!(next));
    payload.insert("createdByAgent".into(), json!(false));
    if let Some(t) = &title {
        payload.insert("title".into(), json!(t));
    }
    let summary = format!("Update Google Doc {id}");
    let realtime = RealtimeDeps::publish_only(state.redis().await.ok());
    let queued = match queue_action(
        &state.pg,
        realtime,
        &QueueAction {
            kind: "doc_update",
            summary: &summary,
            payload: &Value::Object(payload),
            agent_model: &agent_model,
            owner_user_id: principal.owner_user_id.as_deref(),
            is_org: principal.is_org,
            principal_kind: principal.kind.as_str(),
        },
    )
    .await
    {
        Ok(q) => q,
        Err(e) => return Ok(internal("[integrations/google/agent/docs] queue failed", e)),
    };
    Ok(super::integrations_google_agent_queue::answer_queued(
        &state.pg,
        queued,
        "An edit to this doc is already waiting for approval — nothing new queued.",
        "Queued — waiting for the owner to approve before the doc changes. If this agent created the doc, a person has edited it since, so it is theirs now.",
        "Queued — waiting for an admin to approve before the doc changes. If this agent created the doc, a person has edited it since, so it is theirs now.",
    )
    .await)
}

pub async fn get_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Drive") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    let q = query_pairs(uri.query())
        .get("q")
        .cloned()
        .unwrap_or_default();
    match search_files_fulltext(&google.token, &q, 20).await {
        Ok(files) => Ok(Json(json!({ "files": files })).into_response()),
        Err(e) => Ok(google_fail(e, "Drive")),
    }
}

pub async fn get_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(denied) = refuse_legacy(&caller, "Google Drive") {
        return Ok(denied);
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    // What an agent can pull out of Drive is what a person can push in: the
    // upload limit, checked on Content-Length and again on the bytes.
    match read_file_media_with_token(&google.token, &id, talaria_uploads::MAX_BYTES).await {
        Ok(bytes) => match String::from_utf8(bytes.clone()) {
            Ok(text) => {
                Ok(Json(json!({ "id": id, "encoding": "utf-8", "text": text })).into_response())
            }
            Err(_) => Ok(Json(json!({
                "id": id,
                "encoding": "base64",
                "text": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
            .into_response()),
        },
        Err(e) => Ok(google_fail(e, "Drive")),
    }
}

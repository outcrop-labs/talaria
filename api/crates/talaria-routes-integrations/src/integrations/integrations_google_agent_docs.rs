// Agent Google Docs. Reads are free. Creating a new doc is immediate — it
// destroys nothing. Updating a doc the agent did not create queues a
// `doc_update` for a human. Updating one this agent created is immediate.
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
    create_doc_with_token, get_doc_with_token, read_file_media_with_token, search_files_fulltext,
    update_doc_with_token,
};
use talaria_api_facades::google::errors::google_fail;
use talaria_api_facades::google::oauth::query_pairs;
use talaria_api_facades::google::pending_actions::{QueueAction, queue_action};
use talaria_body::{optional_boolean_member, optional_max_string_member, parse, string_member};
use talaria_error::{house_error, house_error_msg, internal, object_or_400};
use talaria_realtime_watch::RealtimeDeps;
use talaria_state::AppState;

const NOT_CONNECTED: &str =
    "No Google account is connected for this agent (its owner, or the org account).";

fn not_connected() -> Response {
    house_error_msg(StatusCode::CONFLICT, "not_connected", NOT_CONNECTED)
}

async fn agent_owns_doc(
    pg: &sqlx::PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from agent_created_google_docs where file_id = $1 and agent_model = $2",
    )
    .bind(file_id)
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

async fn record_created(
    pg: &sqlx::PgPool,
    agent_model: &str,
    file_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into agent_created_google_docs (file_id, agent_model) values ($1, $2) \
         on conflict (file_id) do nothing",
    )
    .bind(file_id)
    .bind(agent_model)
    .execute(pg)
    .await?;
    Ok(())
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
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    let doc = match create_doc_with_token(
        &google.token,
        &title,
        &markdown,
        folder_id.as_deref().filter(|s| !s.is_empty()),
    )
    .await
    {
        Ok(d) => d,
        Err(e) => return Ok(google_fail(e, "Docs")),
    };
    if let Err(e) = record_created(&state.pg, &agent_model, &doc.id).await {
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
        "message": "Created. This agent can edit it without another approval; a human-owned doc still queues.",
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
    let owns = match agent_owns_doc(&state.pg, &agent_model, &id).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(internal(
                "[integrations/google/agent/docs] ownership read failed",
                e,
            ));
        }
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let Some(google) =
        resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(caller), now_ms()).await
    else {
        return Ok(not_connected());
    };
    let next = if append {
        match get_doc_with_token(&google.token, &id).await {
            Ok(current) => format!("{}\n{}", current.markdown, markdown),
            Err(e) => return Ok(google_fail(e, "Docs")),
        }
    } else {
        markdown
    };
    if owns {
        return match update_doc_with_token(&google.token, &id, &next, title.as_deref()).await {
            Ok(doc) => Ok(Json(json!({
                "id": doc.id,
                "url": doc.url,
                "name": doc.name,
                "mimeType": doc.mime_type,
                "createdByAgent": true,
                "message": "Updated. This agent created the doc, so the edit was not queued.",
            }))
            .into_response()),
            Err(e) => Ok(google_fail(e, "Docs")),
        };
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
        "Queued — waiting for the owner to approve before the doc changes.",
        "Queued — waiting for an admin to approve before the doc changes.",
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
    match read_file_media_with_token(&google.token, &id).await {
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

// GET /api/conversations/{id}/files — the session's files, for the Work pane.
//
// WHAT THE PANE SHOWS. A work session's pane is the files the session has
// touched: every Talaria document and Google file a chip on one of its
// messages names (the agent created, read or edited it), then what the
// teammate opened there themselves (`pinned_files`), then — for a work session
// from before this — the living document it was created with. Derived from
// the rows on every read (`talaria_chips::session_files`), so nothing has to
// be kept in step: a file is in the session because the session says so.
//
// THE CONVERSATION'S OWN GATE, and each artifact's own. Access is
// `accessible_conversation` — the predicate every conversation route asks —
// and inaccessible answers the same 404 as nonexistent. Then every artifact
// passes the read check the artifact GET makes for a signed-in person, so a
// collaborator never learns the title of a private document someone else's
// turn linked; a deleted one simply is not found.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::PgPool;

use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_read, granted_item_ids};
use talaria_artifacts::{ArtifactHead, artifact_heads, plan_doc_for};
use talaria_chips::session_files::{
    ArtifactFacts, MAX_SESSION_FILES, SessionFile, TrailMessage, artifact_ids, candidates,
    session_files,
};
use talaria_conversations::{accessible_conversation, conversation_file_trail};
use talaria_error::{house_error, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

/// A read that failed while listing a session's files: which read, and why.
/// The route turns it into its bare 500; the agent door into its own.
pub(crate) struct FilesError(pub &'static str, pub sqlx::Error);

/// THE READ CHECK, the artifact GET's: owner, org/public, or a grant on a
/// private one (direct or through a team) — `granted_item_ids` is that grant
/// leg for every artifact at once, read only when a private one is in the
/// list. Shared with the agent door that opens a file in the pane, so a file
/// it opens for a person is a file this route lists for them.
pub(crate) async fn readable_for(
    pg: &PgPool,
    user_id: &str,
    who: Option<&str>,
    heads: Vec<ArtifactHead>,
) -> Result<HashMap<String, ArtifactFacts>, sqlx::Error> {
    let granted = if heads.iter().any(|h| h.guarded.visibility == "private") {
        granted_item_ids(pg, ITEM_ARTIFACT, user_id).await?
    } else {
        HashSet::new()
    };
    Ok(heads
        .into_iter()
        .filter(|h| granted.contains(&h.id) || can_read(&h.guarded, Some(user_id), who, &[], &[]))
        .map(|h| {
            (
                h.id,
                ArtifactFacts {
                    kind: h.kind,
                    title: h.title,
                },
            )
        })
        .collect())
}

/// The session's files as one person sees them — the whole body of the GET
/// below once access is settled, and what the agent door lists when it is
/// asked for the session's files. `kind` is the conversation's own.
pub(crate) async fn session_files_for(
    pg: &PgPool,
    user_id: &str,
    who: Option<&str>,
    conversation_id: &str,
    kind: &str,
) -> Result<Vec<SessionFile>, FilesError> {
    let (rows, pinned) = conversation_file_trail(pg, conversation_id)
        .await
        .map_err(|e| FilesError("[files] trail read failed", e))?;
    // Only a work session lists a living document — a plan's pane shows its
    // plan document on its own, and no other kind has one.
    let living = if kind == "work" {
        plan_doc_for(pg, conversation_id)
            .await
            .map_err(|e| FilesError("[files] living doc read failed", e))?
            .map(|a| a.id)
    } else {
        None
    };
    let trail: Vec<TrailMessage> = rows
        .into_iter()
        .map(|(created_at, chips, requested_by)| TrailMessage {
            created_at,
            chips,
            requested_by,
        })
        .collect();
    let found = candidates(&trail, &pinned, living.as_deref());
    let heads = artifact_heads(pg, &artifact_ids(&found))
        .await
        .map_err(|e| FilesError("[files] artifact read failed", e))?;
    let readable = readable_for(pg, user_id, who, heads)
        .await
        .map_err(|e| FilesError("[files] grants read failed", e))?;
    Ok(session_files(found, &readable, MAX_SESSION_FILES))
}

// doc: The session's files, oldest first appearance first: every Talaria
// doc: document and Google file the conversation's chips name (`touched`, with
// doc: `lastTouchedAt` the newest message naming it and `touchedFor` the person
// doc: that turn was for), then the teammate's pins (`pinned`), then a legacy
// doc: work session's living document (`living`). Artifacts the caller cannot
// doc: read are left out; at most 100 files.

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[files] access read failed", e)),
    };
    let Some(conv) = conv else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let who = who_of(&user);
    match session_files_for(&state.pg, &user.id, who.as_deref(), &id, &conv.kind).await {
        Ok(files) => Ok(Json(json!({ "kind": conv.kind, "files": files })).into_response()),
        Err(FilesError(context, e)) => Ok(internal(context, e)),
    }
}

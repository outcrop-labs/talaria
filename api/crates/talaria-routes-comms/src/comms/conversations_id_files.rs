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

use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_read, granted_item_ids};
use talaria_artifacts::{artifact_heads, plan_doc_for};
use talaria_chips::session_files::{
    ArtifactFacts, MAX_SESSION_FILES, TrailMessage, artifact_ids, candidates, session_files,
};
use talaria_conversations::{accessible_conversation, conversation_file_trail};
use talaria_error::{house_error, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

// doc: The session's files, oldest first appearance first: every Talaria
// doc: document and Google file the conversation's chips name (`touched`, with
// doc: `lastTouchedAt` the newest message naming it), then the teammate's pins
// doc: (`pinned`), then a legacy work session's living document (`living`).
// doc: Artifacts the caller cannot read are left out; at most 100 files.

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
    let (rows, pinned) = match conversation_file_trail(&state.pg, &id).await {
        Ok(t) => t,
        Err(e) => return Ok(internal("[files] trail read failed", e)),
    };
    // Only a work session lists a living document — a plan's pane shows its
    // plan document on its own, and no other kind has one.
    let living = if conv.kind == "work" {
        match plan_doc_for(&state.pg, &id).await {
            Ok(doc) => doc.map(|a| a.id),
            Err(e) => return Ok(internal("[files] living doc read failed", e)),
        }
    } else {
        None
    };
    let trail: Vec<TrailMessage> = rows
        .into_iter()
        .map(|(created_at, chips)| TrailMessage { created_at, chips })
        .collect();
    let found = candidates(&trail, &pinned, living.as_deref());

    // The read check, the artifact GET's: owner, org/public, or a grant on a
    // private one (direct or through a team) — `granted_item_ids` is that
    // grant leg for every artifact at once, read only when a private one is
    // in the list.
    let heads = match artifact_heads(&state.pg, &artifact_ids(&found)).await {
        Ok(h) => h,
        Err(e) => return Ok(internal("[files] artifact read failed", e)),
    };
    let granted = if heads.iter().any(|h| h.guarded.visibility == "private") {
        match granted_item_ids(&state.pg, ITEM_ARTIFACT, &user.id).await {
            Ok(g) => g,
            Err(e) => return Ok(internal("[files] grants read failed", e)),
        }
    } else {
        HashSet::new()
    };
    let who = who_of(&user);
    let readable: HashMap<String, ArtifactFacts> = heads
        .into_iter()
        .filter(|h| {
            granted.contains(&h.id)
                || can_read(&h.guarded, Some(&user.id), who.as_deref(), &[], &[])
        })
        .map(|h| {
            (
                h.id,
                ArtifactFacts {
                    kind: h.kind,
                    title: h.title,
                },
            )
        })
        .collect();

    let files = session_files(found, &readable, MAX_SESSION_FILES);
    Ok(Json(json!({ "kind": conv.kind, "files": files })).into_response())
}

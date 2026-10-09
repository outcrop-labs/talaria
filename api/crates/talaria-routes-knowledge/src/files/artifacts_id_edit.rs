// /api/artifacts/{id}/edit — change part of a document without resending it.
//
// WHY THIS IS A SEPARATE ROUTE from the PUT. The PUT's contract is "here is
// the new body", and that is the right contract for a person's editor, which
// holds the whole document in a textarea and knows every byte of it. It is the
// WRONG contract for an agent: `update_document` demanded the full markdown
// back, so a one-line fix was billed for the entire document, any section the
// model did not bother to re-emit was silently dropped, and whatever a person
// typed into the same document while the agent was thinking was overwritten.
// This route takes the edit instead of the result.
//
// 409, NOT 400, WHEN A MATCH FAILS. A malformed request and a request whose
// anchor is not in the document are different problems with different fixes,
// and the agent on the other end is the one that has to tell them apart: 400
// means "your JSON was wrong", 409 means "the document is not what you think
// it is — read it again". The sentence from `talaria_doc_edit` rides along,
// because it is written to be read by the model that has to recover.
//
// AUTHORITY AND INDEXING ARE THE PUT'S, deliberately — `body_editor` and
// `reindex_content` are shared with it. Two answers to "may this caller
// rewrite this artifact" is how the tool plane and the UI come to disagree,
// and an edit that skipped the reindex would leave agents retrieving a
// paragraph the document no longer contains.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::files::artifacts_id::{body_editor, reindex_content};
use talaria_artifacts::{SaveArtifactPatch, get_artifact, save_artifact};
use talaria_body::parse;
use talaria_doc_edit::{apply, wire::edits_from};
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let edits = match edits_from(obj) {
        Ok(e) => e,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
        }
        Err(e) => return Ok(internal("[artifacts] read failed on edit", e)),
    };
    // THE KINDS THIS CAN BE HONEST ABOUT. A sheet's body is a JSON grid
    // (`JSON.stringify(rows)`), so a find-and-replace inside it would corrupt
    // the structure rather than edit a cell — and a file artifact's bytes live
    // in storage, not in `body`. Both say which tool to use instead rather
    // than failing on a match that could never have worked.
    match artifact.kind.as_str() {
        "doc" | "microsite" => {}
        "sheet" => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "this is a spreadsheet — its body is a JSON grid, so a text edit would corrupt it. \
                 Use update_document with rows",
            ));
        }
        other => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("a {other} artifact has no editable text body"),
            ));
        }
    }
    let editor = body_editor(&state, &headers, &artifact).await?;
    let applied = match apply(&artifact.body, &edits) {
        Ok(a) => a,
        // The document's state is the reason, so the status says conflict and
        // the body carries the engine's own sentence.
        Err(e) => return Ok(house_error(StatusCode::CONFLICT, &e.to_string())),
    };
    // NO SAVE WHEN NOTHING MOVED. Every content change snapshots a version, so
    // an edit whose replacement equals what was already there would spend a
    // version to record that nothing happened — and the history is what a
    // person reads to see what the agent did.
    if applied.body == artifact.body {
        return Ok(Json(json!({
            "artifact": artifact,
            "changes": changes_json(&applied.changes),
            "unchanged": true,
        }))
        .into_response());
    }
    let updated = match save_artifact(
        &state.pg,
        &id,
        SaveArtifactPatch {
            body: Some(applied.body.as_str()),
            ..Default::default()
        },
        &editor.actor,
    )
    .await
    {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[artifacts] save failed on edit", e)),
    };
    reindex_content(&state, &id, &updated);
    Ok(Json(json!({
        "artifact": updated,
        "changes": changes_json(&applied.changes),
    }))
    .into_response())
}

/// What each edit did — the sentence the chat stream shows instead of making a
/// person diff two versions to find out.
fn changes_json(changes: &[talaria_doc_edit::Change]) -> Value {
    json!(
        changes
            .iter()
            .map(|c| json!({ "label": c.label, "added": c.added, "removed": c.removed }))
            .collect::<Vec<_>>()
    )
}

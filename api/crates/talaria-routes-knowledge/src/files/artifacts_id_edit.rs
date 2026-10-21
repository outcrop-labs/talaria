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
// THE SAVE CARRIES A VERSION PRECONDITION. Read → apply → save is a window,
// and a PUT from the living-document pane that lands inside it used to be
// overwritten by the agent's whole-body save — "whatever a person typed while
// the agent was thinking" was fixed only for the model-thinking window, not
// for this one. The save now binds the `updated_at` the read carried and
// writes only if the row still has it; when it has moved, the route re-reads
// and re-applies once (the match-once rule makes that safe), and if it moves
// again it answers 409 with the sentence this route already speaks: the
// document is not what you think it is — read it again.
//
// AUTHORITY AND INDEXING ARE THE PUT'S, deliberately — `body_editor` and
// `reindex_content` are shared with it. Two answers to "may this caller
// rewrite this artifact" is how the tool plane and the UI come to disagree,
// and an edit that skipped the reindex would leave agents retrieving a
// paragraph the document no longer contains.
//
// THE ANSWER IS SLIM, AND THE CHIP LANDS FROM HERE. Both for the same
// reason: a tool result reaching the platform from a Hermes agent has been
// clamped to 4000 chars by the plugin that reports it. This route used to
// answer with the whole artifact — body first — so an edit to any document
// over about 3 KB arrived at the chip reader as a prefix of JSON, and the
// chat row showed nothing at all about the edit. The body is the one thing
// the caller already has (it just edited it), so it is not sent back; and
// when the caller is an agent the document chip is landed on its live turn
// from this side of the wire, where the change list exists in full. The
// tool-events door still lands the same chip from the result (same id, so
// it merges) — a fallback for every other producer of an edit.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::files::artifacts_id::{body_editor, reindex_content};
use talaria_artifacts::{ConditionalSave, get_artifact, save_artifact_body_if_unchanged};
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
    let mut artifact = artifact;
    let mut moved_once = false;
    let (updated, changes) = loop {
        let applied = match apply(&artifact.body, &edits) {
            Ok(a) => a,
            // The document's state is the reason, so the status says conflict
            // and the body carries the engine's own sentence.
            Err(e) => return Ok(house_error(StatusCode::CONFLICT, &e.to_string())),
        };
        // NO SAVE WHEN NOTHING MOVED. Every content change snapshots a
        // version, so an edit whose replacement equals what was already there
        // would spend a version to record that nothing happened — and the
        // history is what a person reads to see what the agent did.
        let changes = changes_json(&applied.changes);
        if applied.body == artifact.body {
            return Ok(Json(json!({
                "changes": changes,
                "artifact": slim(&artifact),
                "unchanged": true,
            }))
            .into_response());
        }
        match save_artifact_body_if_unchanged(
            &state.pg,
            &id,
            &applied.body,
            &editor.actor,
            &artifact.updated_at,
        )
        .await
        {
            Ok(ConditionalSave::Saved(a)) => break (*a, changes),
            Ok(ConditionalSave::Missing) => {
                return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
            }
            Ok(ConditionalSave::Moved) if !moved_once => {
                // Someone saved between the read and this write. Re-read and
                // re-apply once: the edit is anchored, not positional, so it
                // lands on the new body or refuses honestly.
                moved_once = true;
                artifact = match get_artifact(&state.pg, &id).await {
                    Ok(Some(a)) => a,
                    Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
                    Err(e) => return Ok(internal("[artifacts] re-read failed on edit", e)),
                };
            }
            Ok(ConditionalSave::Moved) => {
                return Ok(house_error(
                    StatusCode::CONFLICT,
                    "the document moved while this edit was being applied — read it again and retry",
                ));
            }
            Err(e) => return Ok(internal("[artifacts] save failed on edit", e)),
        }
    };
    reindex_content(&state, &id, &updated);
    land_document_chip(&state, &editor, &updated, &changes);
    // WHAT THE EDIT DID FIRST, then the artifact without its body. The order
    // is load-bearing on one wire: an agent's plugin clamps the tool result
    // it reports to 4000 chars, so whatever this object says first is what
    // survives the trip — and the changes are the part nobody downstream can
    // reconstruct, where the id and the title are two short fields that fit
    // behind them.
    Ok(Json(json!({ "changes": changes, "artifact": slim(&updated) })).into_response())
}

/// The artifact this route answers with: everything the caller might read
/// EXCEPT the body.
///
/// The body is the one field nobody on this door needs — the caller is the
/// one that just edited it, `get_document` re-reads it in one call, and the
/// living-document pane reloads the artifact itself after an edit rather
/// than taking it from this response. Sending it put a document-sized
/// string in front of `changes` on a wire with a 4000-char clamp on it,
/// which cost the chat row the whole change list.
fn slim(a: &talaria_artifacts::Artifact) -> Value {
    json!({
        "id": a.id,
        "kind": a.kind,
        "title": a.title,
        "updatedAt": a.updated_at,
    })
}

/// AN AGENT'S EDIT, ON ITS LIVE TURN. The chip carries what the edit did, so
/// a person who was not watching can see what moved without opening the
/// document.
///
/// FROM HERE RATHER THAN FROM THE TOOL RESULT, because the change list only
/// survives in full on this side: the agent's plugin clamps every tool
/// result to 4000 chars, so the round trip through `/api/agents/tool-events`
/// is best-effort by construction. It still runs, and lands the same chip id
/// — whichever arrives second merges onto the first instead of adding a row.
///
/// DETACHED, like the reindex above and for the same reason: the save has
/// already happened, and failing this response over a chip would tell the
/// agent its edit did not land. `human: None` IS the agent case — the one
/// `body_editor` sets for an agent key — because a person editing their own
/// document has the document open and needs no chip about it.
fn land_document_chip(
    state: &AppState,
    editor: &crate::files::artifacts_id::Editor,
    updated: &talaria_artifacts::Artifact,
    changes: &Value,
) {
    if editor.human.is_some() {
        return;
    }
    let chip = talaria_chips::document_chip(&updated.id, Some(&updated.title), Some(changes));
    let (state, agent) = (state.clone(), editor.actor.clone());
    tokio::spawn(async move {
        let redis = state.redis().await.ok();
        match talaria_chips::surface_for_agent(&state.pg, redis, &agent, vec![chip]).await {
            // No live turn: a work-session or cron edit has no chat row to
            // land on, which is ordinary rather than a failure.
            Ok(_) => {}
            Err(e) => tracing::warn!("[artifacts] edit chip surface failed for {agent}: {e}"),
        }
    });
}

/// What each edit did — the sentence the chat stream shows instead of making a
/// person diff two versions to find out. Each row is `{label, added,
/// removed}` — `added` and `removed` are LINES, the way the card and the
/// chip render them — plus, for a text replacement, `preview: {old, new}`:
/// the clipped, one-line before/after the chat row shows.
///
/// PREVIEW IS OMITTED, NOT NULL, when the engine has none (a section edit,
/// whose before/after is the whole section). A key that is sometimes null
/// makes every reader downstream test for two different absences.
/// At most this many rows ride back — the chip's own ceiling. The MCP tool
/// admits fifty edits in one call; rows past the twentieth would only be
/// clamp pressure on the agent's copy of its result (measured against the
/// pretty-printed form the MCP layer sends), and the chip never shows them.
const MAX_CHANGE_ROWS: usize = 20;

fn changes_json(changes: &[talaria_doc_edit::Change]) -> Value {
    json!(
        changes
            .iter()
            .take(MAX_CHANGE_ROWS)
            .map(|c| {
                let mut row = json!({ "label": c.label, "added": c.added, "removed": c.removed });
                if let Some(p) = &c.preview
                    && let Some(obj) = row.as_object_mut()
                {
                    obj.insert("preview".into(), json!({ "old": p.old, "new": p.new }));
                }
                row
            })
            .collect::<Vec<_>>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_doc_edit::{Change, ChangePreview};

    fn artifact(body: &str) -> talaria_artifacts::Artifact {
        talaria_artifacts::Artifact {
            id: "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2".into(),
            kind: "doc".into(),
            title: "Runbook".into(),
            icon: None,
            body: body.into(),
            content_type: None,
            storage_ref: None,
            visibility: "org".into(),
            edit_policy: "org".into(),
            public_slug: None,
            official: false,
            kb_doc_id: None,
            folder_id: None,
            owner_user_id: None,
            rag_routing: "auto".into(),
            google_file_id: None,
            google_file_url: None,
            created_by: None,
            updated_by: Some("agent:scribe".into()),
            created_at: "2026-10-01T09:00:00.000Z".into(),
            updated_at: "2026-10-16T11:22:33.000Z".into(),
        }
    }

    /// THE SHAPE THAT FITS THROUGH THE CLAMP. This route's answer is the
    /// one an agent's plugin reports onward, cut to 4000 chars — so the
    /// body must not ride along and the changes must come first. With the
    /// whole artifact in front of them, every edit to a document over about
    /// 3 KB arrived unparseable and the chat row said nothing at all.
    ///
    /// The object below is the handler's own literal, key for key; the
    /// generated reference (`docs/api/files.md`) prints those keys in the
    /// same order, so a drift between the three is visible in two places.
    #[test]
    fn the_answer_leads_with_the_changes_and_never_carries_the_body() {
        let changes = changes_json(&[
            Change {
                label: "the old line".into(),
                added: 1,
                removed: 1,
                preview: Some(ChangePreview {
                    old: "the old line".into(),
                    new: "the new line".into(),
                }),
            },
            Change {
                label: "## Rollback".into(),
                added: 4,
                removed: 9,
                preview: None,
            },
        ]);
        let out =
            json!({ "changes": changes, "artifact": slim(&artifact(&"body text. ".repeat(500))) });
        let text = serde_json::to_string(&out).expect("plain data");
        assert!(text.starts_with(r#"{"changes":"#), "{text}");
        assert!(!text.contains("body text"), "the body never rides along");
        // The wire is the MCP layer's PRETTY-PRINTED form (JSON.stringify
        // with a 2-space indent), so that is the form measured against the
        // plugin's 4000-char clamp.
        let pretty = serde_json::to_string_pretty(&out).expect("plain data");
        assert!(
            pretty.chars().count() < 4_000,
            "and the whole answer fits inside the plugin's clamp: {} chars",
            pretty.chars().count()
        );

        // What a caller DOES read: which document, what it is called, and
        // when it moved.
        assert_eq!(
            out["artifact"]["id"],
            "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2"
        );
        assert_eq!(out["artifact"]["kind"], "doc");
        assert_eq!(out["artifact"]["title"], "Runbook");
        assert_eq!(out["artifact"]["updatedAt"], "2026-10-16T11:22:33.000Z");
        assert!(out["artifact"].get("body").is_none(), "{out}");

        // The rows the card and the chip render: LINES, and a preview only
        // where there is an honest one.
        assert_eq!(out["changes"][0]["added"], 1);
        assert_eq!(out["changes"][0]["removed"], 1);
        assert_eq!(out["changes"][0]["preview"]["new"], "the new line");
        assert_eq!(out["changes"][1]["label"], "## Rollback");
        assert!(out["changes"][1].get("preview").is_none(), "{out}");
    }

    /// AND THE SAME CHIP THE ROUND TRIP WOULD LAND. The route lands it
    /// itself because the change list only survives in full on this side;
    /// the two must be one chip, which means one id.
    #[test]
    fn the_route_chip_is_the_one_the_tool_result_would_have_landed() {
        let a = artifact("whatever");
        let changes = changes_json(&[Change {
            label: "the old line".into(),
            added: 1,
            removed: 1,
            preview: None,
        }]);
        let from_route = talaria_chips::document_chip(&a.id, Some(&a.title), Some(&changes));
        let from_result = talaria_chips::chips_from_tool(
            "edit_document",
            &format!(r#"{{"documentId":"{}"}}"#, a.id),
            &serde_json::to_string(&json!({ "changes": changes, "artifact": slim(&a) }))
                .expect("plain data"),
        );
        let round_trip = from_result
            .iter()
            .find(|c| c["entity"] == "document")
            .expect("the fallback still lands one");
        assert_eq!(from_route, *round_trip, "one chip, not two rows");
        assert_eq!(from_route["id"], format!("link:/artifacts?a={}", a.id));
        assert_eq!(from_route["changes"][0]["label"], "the old line");
    }
}

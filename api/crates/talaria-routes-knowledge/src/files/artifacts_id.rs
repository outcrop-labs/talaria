// /api/artifacts/{id}. One artifact: read/edit gated by its audience,
// sharing owner-only, agents (by key) only edit content when granted the
// Editor role — a personal assistant READS its owner's artifacts the way it
// reads their docs (can_read_agent's owner arm), and edit stays grant-only.
// The PUT is the plane's whole state machine — content edits, sharing,
// official curation and brain routing all land here, in this order.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::knowledge::kb_spaces_id::{editors_json, parse_editors};
use talaria_agent_auth::agent_caller;
use talaria_api_facades::kb::perms::{
    ITEM_ARTIFACT, can_edit_human, can_govern, can_read, list_editors, set_editors,
};
use talaria_api_facades::retrieval::artifact_routing::apply_artifact_routing;
use talaria_api_facades::retrieval::{embed, qdrant};
use talaria_artifacts::agent_access::{agent_edits_artifact, agent_reads_artifact};
use talaria_artifacts::{
    ConditionalSave, SaveArtifactPatch, delete_artifact, get_artifact, guarded, index_plan_doc,
    save_artifact, save_artifact_content_if_unchanged, set_artifact_official, set_artifact_routing,
    targets_for_artifact,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    optional_boolean_member, optional_enum_member, optional_max_string_member, parse,
    present_nullable_max_string_member, present_nullable_uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user, who_of};
use talaria_state::AppState;
use talaria_uploads::{UploadViewer, can_access_upload};

struct PutBody {
    title: Option<String>,
    body: Option<String>,
    icon: Option<Option<String>>,
    storage_ref: Option<Option<String>>,
    content_type: Option<Option<String>>,
    folder_id: Option<Option<String>>,
    visibility: Option<String>,
    edit_policy: Option<String>,
    editors: Option<Vec<talaria_api_facades::kb::perms::EditorGrant>>,
    official: Option<bool>,
    rag_routing: Option<String>,
    /// The `updatedAt` the caller's editing surface was seeded from — see
    /// `save_artifact_content_if_unchanged`. Absent: the old replace.
    base_updated_at: Option<String>,
}

fn parse_put_body(obj: &serde_json::Map<String, Value>) -> Result<PutBody, String> {
    Ok(PutBody {
        title: optional_max_string_member(obj, "title", 200)?,
        body: optional_max_string_member(obj, "body", 2_000_000)?,
        icon: present_nullable_max_string_member(obj, "icon", 16)?,
        storage_ref: present_nullable_uuid_member(obj, "storageRef")?,
        content_type: present_nullable_max_string_member(obj, "contentType", 200)?,
        folder_id: present_nullable_uuid_member(obj, "folderId")?,
        visibility: optional_enum_member(obj, "visibility", &["private", "org", "public"])?,
        edit_policy: optional_enum_member(obj, "editPolicy", &["owner", "org", "restricted"])?,
        editors: parse_editors(obj.get("editors"))?,
        official: optional_boolean_member(obj, "official")?,
        rag_routing: optional_max_string_member(obj, "ragRouting", 60)?,
        base_updated_at: valid_base(optional_max_string_member(obj, "baseUpdatedAt", 40)?)?,
    })
}

/// `baseUpdatedAt` — the version an editor's buffer started from, as the
/// `updatedAt` it read (ISO 8601). Shared with `/api/kb/docs/{id}`. A value
/// that is not a timestamp is the caller's mistake, said at 400, never a
/// cast failure at 500 inside the conditional update.
pub(crate) fn valid_base(base: Option<String>) -> Result<Option<String>, String> {
    if let Some(b) = &base
        && chrono::DateTime::parse_from_rfc3339(b).is_err()
    {
        return Err("baseUpdatedAt must be an ISO 8601 timestamp (the updatedAt you read)".into());
    }
    Ok(base)
}

/// The refusal's sentence — the editor shows its own line, so this one is
/// written for anything else that reads the 409.
pub(crate) const STALE_BASE: &str = "this document changed since your copy was loaded — \
     reload it (or review it in version history) before saving over it";

fn not_found() -> Response {
    house_error(StatusCode::NOT_FOUND, "not found")
}

/// Wire sentence for a transient failure on GET /api/artifacts/{id}.
/// A missing row is `not_found` (404). This is only the Err arms. The
/// engine error stays in the log — artifact id, arm, and the error — and
/// never reaches the wire. `list_documents` returns every readable artifact
/// with its full body, which is how a single-read failure is recovered.
const READ_RETRYABLE: &str =
    "artifact exists, read failed — retryable. Recover the body with list_documents.";

fn read_failed(id: &str, arm: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!("[artifacts] {arm} artifact={id} arm={arm}: {e}");
    house_error(StatusCode::INTERNAL_SERVER_ERROR, READ_RETRYABLE)
}

/// Write-method arms on this route. Same log contract (id, arm, error);
/// the wire stays the bare 500 those callers already match.
fn arm_failed(context: &str, id: &str, arm: &str, e: impl std::fmt::Display) -> Response {
    internal(&format!("{context} artifact={id} arm={arm}"), e)
}

/// WHO MAY CHANGE THIS ARTIFACT'S BODY, resolved once.
///
/// TWO ROUTES ASK IT: the PUT (which sends a whole new body) and
/// `/{id}/edit` (which sends a surgical patch). They are the same question —
/// "may this caller rewrite this artifact's content, and under what name is
/// the change stamped" — and two implementations of it is how they come to
/// disagree, which on this plane means a tool reaching a document the UI
/// would refuse. Sharing, official curation and brain routing are NOT here:
/// those are the PUT's own state machine, and `/edit` cannot touch them.
pub(crate) struct Editor {
    /// The name the change is attributed to.
    pub actor: String,
    /// May govern the artifact (sharing, official, routing) — always false
    /// for an agent key, which never governs.
    pub owner: bool,
    /// The session user, when a person is the caller. `None` means an agent
    /// key, and the PUT reads it that way to strip the fields an agent may
    /// not move.
    pub human: Option<talaria_session::SessionUser>,
}

/// KEEP THE RETRIEVABLE COPY CURRENT after a content change: `auto` goes
/// through the plan-doc activity flow, an explicit brain re-indexes there.
///
/// SHARED WITH `/{id}/edit` for one reason — a surgical edit changes the body
/// exactly as a PUT does, so an edit that skipped this would leave agents
/// retrieving the paragraph the document no longer contains. Detached, and its
/// errors are swallowed: the save already succeeded, and failing the response
/// over the index would tell the caller their edit did not land.
pub(crate) fn reindex_content(state: &AppState, id: &str, updated: &talaria_artifacts::Artifact) {
    let u = updated.clone();
    if !u.rag_routing.is_empty() && u.rag_routing != "auto" {
        let (pg, qd, ed) = (state.pg.clone(), qdrant::real_deps(), embed::real_deps());
        tokio::spawn(async move {
            apply_artifact_routing(&pg, &qd, &ed, &u).await;
        });
    } else {
        let (pg, qd, ed, id) = (
            state.pg.clone(),
            qdrant::real_deps(),
            embed::real_deps(),
            id.to_string(),
        );
        tokio::spawn(async move {
            if let Ok(targets) = targets_for_artifact(&pg, &id).await
                && let Some((_, plan_id)) = targets.iter().find(|(tt, _)| tt == "plan")
            {
                let _ = index_plan_doc(&pg, &qd, &ed, &u, plan_id).await;
            }
        });
    }
}

/// An artifact as an AGENT's write is answered: which document, what kind,
/// what it is called and when it moved — everything except the body. Shared
/// by the create, the PUT and `/{id}/edit`, so the three writes an agent makes
/// answer in one shape. `id` first.
///
/// THE BODY IS THE ONE FIELD NOBODY ON THESE DOORS NEEDS, and the one that
/// broke them. The agent just wrote it, and `get_document` re-reads it in one
/// call. And an agent's answer reaches the chip reader pretty-printed, inside
/// Hermes' `{"result": "…"}` envelope, cut to 4000 chars by its plugin. A
/// document-sized string in it pushed the id past the cut (the create's
/// answer opened `id, kind, title, icon, body…`, so a markdown body of about
/// 2.5 KB lost the id), and the file never joined the session from the tool
/// event. Without the body the answer is a few hundred characters whatever
/// the document's size.
pub(crate) fn slim(a: &talaria_artifacts::Artifact) -> Value {
    json!({
        "id": a.id,
        "kind": a.kind,
        "title": a.title,
        "updatedAt": a.updated_at,
    })
}

/// AN AGENT'S WRITE, ON ITS LIVE TURN: the document chip that puts the file
/// in the session's pane, landed by the route that made the write. The
/// create, the PUT and `/{id}/edit` call it, each only for an agent key — a
/// person writing their own document has it open and needs no chip about it.
/// `changes` is the edit's change list (the chip shows what moved); a create
/// or a whole-body rewrite has none.
///
/// FROM HERE AS WELL AS FROM THE TOOL RESULT. The tool-events door lands the
/// same chip from the agent's answer, addressed to the conversation the call
/// was made in (`talaria_chips::session_hint`); this side has the write in
/// full, but names no conversation. Same chip id, so whichever arrives second
/// merges onto the first instead of adding a row.
///
/// ONLY ON A LIVE TURN (`surface_on_live_turn`): a document chip opens the
/// file in the pane, and a write with no streaming reply — a cron run, a
/// background job — must not open it in the pane of whoever the agent last
/// spoke to. DETACHED: the write already happened, and failing the response
/// over a chip would tell the agent it did not.
pub(crate) fn land_document_chip(
    state: &AppState,
    agent: &str,
    artifact: &talaria_artifacts::Artifact,
    changes: Option<&Value>,
) {
    let chip = talaria_chips::document_chip(&artifact.id, Some(&artifact.title), changes);
    let (state, agent) = (state.clone(), agent.to_string());
    tokio::spawn(async move {
        let redis = state.redis().await.ok();
        // No live turn is ordinary rather than a failure: a work-session or
        // cron write has no chat row to land on.
        if let Err(e) =
            talaria_chips::surface_on_live_turn(&state.pg, redis, &agent, vec![chip]).await
        {
            tracing::warn!("[artifacts] document chip surface failed for {agent}: {e}");
        }
    });
}

pub(crate) async fn body_editor(
    state: &AppState,
    headers: &HeaderMap,
    artifact: &talaria_artifacts::Artifact,
) -> Result<Editor, Response> {
    let editors = list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id)
        .await
        .map_err(|e| arm_failed("[artifacts] grants read failed", &artifact.id, "grants", e))?;
    let g = guarded(artifact);

    let agent = agent_caller(&state.pg, headers).await?;
    if let Some(agent) = agent {
        // Editor grant — or an admin-elevated assistant on any non-private
        // artifact. The rule is talaria-artifacts' (`agent_edits_artifact`),
        // shared with the GET and the pane door.
        let may = agent_edits_artifact(&state.pg, &agent, &g, &editors)
            .await
            .map_err(|e| {
                arm_failed(
                    "[artifacts] agent edit check failed",
                    &artifact.id,
                    "agent-edit",
                    e,
                )
            })?;
        if !may {
            return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        return Ok(Editor {
            actor: agent.model,
            owner: false,
            human: None,
        });
    }

    let user = require_user(state, headers).await?;
    let who = who_of(&user);
    let team_ids = talaria_teams::team_ids_for_user(&state.pg, &user.id)
        .await
        .map_err(|e| {
            arm_failed(
                "[artifacts] team membership read failed",
                &artifact.id,
                "team-membership-user",
                e,
            )
        })?;
    if !can_edit_human(&g, Some(&user.id), who.as_deref(), &editors, &team_ids) {
        return Err(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    // `canGovern`, not `isOwner` — the same rule kb/docs/{id} already
    // uses, and the reason canGovern exists. Attribution stamps a human
    // owner on what an agent makes, but it cannot promise one: an
    // untraceable caller (or output from before attribution) is ownerless,
    // and strict ownership would leave those files orphans whose sharing
    // literally nobody could change. canGovern hands the ownerless to
    // admins and to whoever may use the agent that wrote them, while
    // owned artifacts — agent-made or not — are owner-only, their
    // owners' to govern.
    let owner = can_govern(&state.pg, &g, &user.id, &user.role, who.as_deref())
        .await
        .map_err(|e| arm_failed("[artifacts] govern check failed", &artifact.id, "govern", e))?;
    Ok(Editor {
        actor: actor_of(&user),
        owner,
        human: Some(user),
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => return Ok(read_failed(&id, "read failed", e)),
    };
    let Some(artifact) = artifact else {
        return Ok(not_found());
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => return Ok(read_failed(&artifact.id, "grants read failed", e)),
    };
    // Agents (over MCP) read org/public artifacts, ones granted to them, and —
    // for a personal assistant — its owner's own (can_read_agent's owner arm).
    let reader = agent_caller(&state.pg, &headers).await?;
    if let Some(reader) = reader {
        match agent_reads_artifact(&state.pg, &reader, &guarded(&artifact), &editors).await {
            Ok(true) => {}
            Ok(false) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
            Err(e) => return Ok(read_failed(&artifact.id, "agent read check failed", e)),
        }
        return Ok(
            Json(json!({ "artifact": artifact, "editors": editors_json(&editors) }))
                .into_response(),
        );
    }
    let user = require_user(&state, &headers).await?;
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(read_failed(
                &artifact.id,
                "team membership read failed (user)",
                e,
            ));
        }
    };
    if !can_read(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    // The PUT's content rule (`body_editor`'s human arm), answered on read, so
    // a surface can leave out what the PUT would refuse (version history's
    // Restore for a viewer).
    let can_edit = can_edit_human(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    );
    let mut wire = serde_json::to_value(&artifact).expect("Artifact serializes");
    if let Some(obj) = wire.as_object_mut() {
        obj.insert("canEdit".into(), json!(can_edit));
    }
    Ok(Json(json!({ "artifact": wire, "editors": editors_json(&editors) })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            return Ok(arm_failed(
                "[artifacts] read failed",
                &id,
                "artifact-row",
                e,
            ));
        }
    };
    let Some(artifact) = artifact else {
        return Ok(not_found());
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let mut body = match parse_put_body(obj) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let Editor {
        actor,
        owner,
        human,
    } = body_editor(&state, &headers, &artifact).await?;
    if let Some(user) = human.as_ref() {
        if body.visibility.as_deref() == Some("public")
            && !matches!(
                talaria_permissions::has_perm(&state.pg, &user.id, &user.role, "artifacts.publish")
                    .await,
                Ok(true)
            )
        {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "no permission to publish to the web",
            ));
        }
        let sharing =
            body.visibility.is_some() || body.edit_policy.is_some() || body.editors.is_some();
        if !owner && sharing {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "not allowed to change sharing",
            ));
        }
        // Routing decides which brain retrieves the content — owner's call.
        if !owner && body.rag_routing.is_some() {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "only the owner can change brain routing",
            ));
        }
    } else {
        // An agent key edits CONTENT and nothing else. Stripped here rather
        // than refused so a tool that sends a whole artifact back does not
        // fail on fields it was never going to be allowed to move.
        body.visibility = None;
        body.edit_policy = None;
        body.editors = None;
        body.official = None;
        body.rag_routing = None;
        // RE-POINTING THE FILE is a read of that upload by everyone who can
        // read this artifact: the upload ACL grants on "referenced by a
        // non-private artifact". So an agent may only point at an upload it
        // can already read itself — a ticket attachment on a board it
        // works, a message in a channel it sits in, its own save.
        if let Some(Some(upload_id)) = &body.storage_ref
            && !can_access_upload(&state.pg, upload_id, UploadViewer::Agent { model: &actor }).await
        {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "storageRef is not an upload this agent can read",
            ));
        }
    }

    // NO SILENT OVERWRITE. An editor's save names the version its buffer
    // started from; its title and body land in one conditional update
    // (`save_artifact_content_if_unchanged`), and a file that moved under the
    // buffer — an agent's update_document, another person's save — is
    // refused with the row as it now is. After authority (a caller who may
    // not edit learns nothing from a 409), before anything else is written.
    let content = body.body.is_some() || body.title.is_some();
    let mut content_saved = false;
    if let (Some(base), Some(new_body)) = (body.base_updated_at.as_deref(), body.body.as_deref()) {
        match save_artifact_content_if_unchanged(
            &state.pg,
            &id,
            body.title.as_deref(),
            new_body,
            &actor,
            base,
        )
        .await
        {
            Ok(ConditionalSave::Saved(_)) => content_saved = true,
            Ok(ConditionalSave::Moved) => {
                let now = match get_artifact(&state.pg, &id).await {
                    Ok(Some(a)) => a,
                    Ok(None) => return Ok(not_found()),
                    Err(e) => {
                        return Ok(arm_failed("[artifacts] re-read failed", &id, "re-read", e));
                    }
                };
                return Ok((
                    StatusCode::CONFLICT,
                    Json(json!({ "error": STALE_BASE, "conflict": true, "artifact": now })),
                )
                    .into_response());
            }
            Ok(ConditionalSave::Missing) => return Ok(not_found()),
            Err(e) => {
                return Ok(arm_failed(
                    "[artifacts] save failed",
                    &id,
                    "save-content",
                    e,
                ));
            }
        }
    }

    if !owner {
        body.official = None;
    }
    if owner
        && let Some(editors) = &body.editors
        && let Err(e) = set_editors(&state.pg, ITEM_ARTIFACT, &id, editors).await
    {
        return Ok(arm_failed(
            "[knowledge] set_editors failed",
            &id,
            "set-editors",
            e,
        ));
    }
    let mut updated = match save_artifact(
        &state.pg,
        &id,
        SaveArtifactPatch {
            // The Patch's title is string-optional, never nullish — absent
            // means "don't touch", never "clear" (the column is not-null).
            // Title and body were already written, conditionally, above.
            title: body.title.as_deref().filter(|_| !content_saved).map(Some),
            body: body.body.as_deref().filter(|_| !content_saved),
            icon: body.icon.as_ref().map(|o| o.as_deref()),
            storage_ref: body.storage_ref.as_ref().map(|o| o.as_deref()),
            content_type: body.content_type.as_ref().map(|o| o.as_deref()),
            folder_id: body.folder_id.as_ref().map(|o| o.as_deref()),
            visibility: body.visibility.as_deref(),
            edit_policy: body.edit_policy.as_deref(),
        },
        &actor,
    )
    .await
    {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(not_found()),
        Err(e) => return Ok(arm_failed("[artifacts] save failed", &id, "save", e)),
    };
    if let Some(official) = body.official
        && official != updated.official
    {
        updated = match set_artifact_official(
            &state.pg,
            &qdrant::real_deps(),
            &embed::real_deps(),
            &id,
            official,
            &actor,
        )
        .await
        {
            Ok(Some(a)) => a,
            Ok(None) => updated,
            Err(e) => {
                return Ok(arm_failed(
                    "[artifacts] officialize failed",
                    &id,
                    "officialize",
                    e,
                ));
            }
        };
        let (pg, actor_, action, target_id, target_label) = (
            state.pg.clone(),
            actor.clone(),
            if official {
                "artifact.officialize"
            } else {
                "artifact.deofficialize"
            },
            id.clone(),
            updated.title.clone(),
        );
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &actor_,
                    action,
                    target_type: "artifact",
                    target_id: Some(&target_id),
                    target_label: Some(&target_label),
                    before: None,
                    after: None,
                },
            )
            .await;
        });
    }
    // Routing change → re-place immediately (and validate the brain).
    if let Some(routing) = body.rag_routing.clone() {
        match set_artifact_routing(&state.pg, &id, &routing, &actor).await {
            Ok(Some(routed)) => {
                updated = routed.clone();
                let (pg, qd, ed, routed) = (
                    state.pg.clone(),
                    qdrant::real_deps(),
                    embed::real_deps(),
                    routed,
                );
                tokio::spawn(async move {
                    apply_artifact_routing(&pg, &qd, &ed, &routed).await;
                });
            }
            Ok(None) => {}
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    }
    if content {
        reindex_content(&state, &id, &updated);
    }
    // AN AGENT'S REWRITE (`update_document`) IS ANSWERED LIKE ITS EDIT: the
    // artifact without its body (`slim`), and the document chip landed from
    // here. The whole body in the answer cost the tool event its id past
    // about 2.5 KB, so a long rewrite of a sheet or a doc never reached the
    // pane at all. No `editors` either: an agent cannot change sharing, never
    // reads the list back, and every grant on a shared document added ~150
    // characters to a wire with a 4000-character cut (one grant beside a
    // 200-character non-ASCII title came to 1,564).
    if human.is_none() {
        land_document_chip(&state, &actor, &updated, None);
        return Ok(Json(json!({ "artifact": slim(&updated) })).into_response());
    }
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &id).await {
        Ok(e) => e,
        Err(e) => {
            return Ok(arm_failed(
                "[artifacts] grants read failed",
                &id,
                "grants",
                e,
            ));
        }
    };
    Ok(Json(json!({ "artifact": updated, "editors": editors_json(&editors) })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            return Ok(arm_failed(
                "[artifacts] read failed",
                &id,
                "artifact-row",
                e,
            ));
        }
    };
    let Some(artifact) = artifact else {
        return Ok(not_found());
    };
    let user = require_user(&state, &headers).await?;
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => {
            return Ok(arm_failed(
                "[artifacts] grants read failed",
                &artifact.id,
                "grants",
                e,
            ));
        }
    };
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(arm_failed(
                "[artifacts] team membership read failed",
                &artifact.id,
                "team-membership-user",
                e,
            ));
        }
    };
    if !can_edit_human(
        &guarded(&artifact),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if let Err(e) = delete_artifact(&state.pg, &qdrant::real_deps(), &embed::real_deps(), &id).await
    {
        return Ok(arm_failed("[artifacts] delete failed", &id, "delete", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_api_facades::kb::perms::EditorGrant;

    const ID: &str = "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2";

    fn artifact(kind: &str, title: &str, body: &str) -> talaria_artifacts::Artifact {
        talaria_artifacts::Artifact {
            id: ID.into(),
            kind: kind.into(),
            title: title.into(),
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
            created_by: Some("agent-model".into()),
            updated_by: Some("agent-model".into()),
            created_at: "2026-01-05T09:00:00.000Z".into(),
            updated_at: "2026-01-05T09:04:12.000Z".into(),
        }
    }

    /// Markdown of exactly `len` characters: headings, bullets and a table,
    /// the shape an agent's brief has.
    fn markdown(len: usize) -> String {
        let mut out = String::new();
        let mut n = 0;
        while out.chars().count() < len {
            n += 1;
            out.push_str(&format!(
                "## Section {n}\n\n- the first point of section {n}, with a [link](https://example.com/{n})\n\
                 - a second point that says a little more about it\n\n| item | owner |\n|---|---|\n| row {n} | ops |\n\n"
            ));
        }
        out.chars().take(len).collect()
    }

    /// A JSON string's body as Python's `json.dumps` writes it with
    /// `ensure_ascii` (the default): `"` and `\` escaped, the short escapes
    /// for the control characters that have them, and every other character
    /// outside printable ASCII as `\uXXXX` — UTF-16 units, lowercase hex.
    fn python_ascii(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                ' '..='~' => out.push(c),
                c => {
                    let mut units = [0u16; 2];
                    for unit in c.encode_utf16(&mut units) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                }
            }
        }
        out
    }

    /// WHAT THE CHIP READER IS HANDED, for an answer this crate gave an agent
    /// — the live wire as FR1's Hermes v0.22.4 probe recorded it: the toolkit
    /// pretty-prints the answer with a 2-space indent (`ok()`), Hermes' MCP
    /// handler wraps that text as `{"result": …}` (`json.dumps`, default
    /// separators, `ensure_ascii`), and the talaria-events plugin cuts the
    /// envelope to 4000 characters (`_clamp`).
    fn as_the_door_reads(answer: &Value) -> String {
        let text = serde_json::to_string_pretty(answer).expect("plain data");
        let enveloped = format!("{{\"result\": \"{}\"}}", python_ascii(&text));
        enveloped.chars().take(4_000).collect()
    }

    /// The document chips the tool-events door reads off that wire, under
    /// the name Hermes registered the tool by.
    fn door_chips(tool: &str, args: &str, answer: &Value) -> Vec<Value> {
        let name = format!("mcp__talaria__{tool}");
        talaria_chips::chips_from_tool(
            talaria_chips::bare_tool_name(&name),
            args,
            &talaria_chips::mcp_result_text(&as_the_door_reads(answer)),
        )
        .into_iter()
        .filter(|c| c["entity"] == "document")
        .collect()
    }

    fn agent_grant() -> Vec<EditorGrant> {
        vec![EditorGrant {
            principal_type: "agent".into(),
            principal_id: "a-fleet-agent-model-name-of-some-length".into(),
            role: "editor".into(),
        }]
    }

    /// AN AGENT'S CREATE AND REWRITE ARE ANSWERED LIKE ITS EDIT: no body, the
    /// id first, and a few hundred characters whatever the document's size —
    /// under 1,500 on the wire even when the title is 200 characters that
    /// `ensure_ascii` turns into six each. The objects below are the
    /// handlers' own literals (`artifacts::post`'s agent branch and the PUT's
    /// agent branch); the live suite reads the real routes' answers. With
    /// the human answer's `editors` beside it, one grant took the rewrite's
    /// wire to 1,564, which is why the agent's answer leaves it out.
    #[test]
    fn an_agents_create_and_rewrite_answers_leave_the_body_out_and_lead_with_the_id() {
        let title = "é".repeat(200);
        let a = artifact("doc", &title, &markdown(4_739));
        let create = json!({ "artifact": slim(&a) });
        let rewrite = json!({ "artifact": slim(&a) });
        for (what, answer) in [("create", &create), ("rewrite", &rewrite)] {
            let artifact = answer["artifact"].as_object().expect("an object");
            assert!(artifact.get("body").is_none(), "{what}: {answer}");
            assert_eq!(
                artifact.keys().next().map(String::as_str),
                Some("id"),
                "{what}: the id leads"
            );
            assert_eq!(
                artifact.keys().map(String::as_str).collect::<Vec<_>>(),
                vec!["id", "kind", "title", "updatedAt"],
                "{what}: the edit route's shape"
            );
            let wire = as_the_door_reads(answer);
            let n = wire.chars().count();
            assert!(n < 1_500, "{what}: {n} chars on the wire");
            assert!(wire.ends_with("\"}"), "{what}: nothing was cut: {wire}");
            assert!(
                !wire.contains("Section 1"),
                "{what}: the body never rides along"
            );
        }
    }

    /// THE DOOR READS EVERY WRITER'S ANSWER WHOLE NOW, and a reader's not at
    /// all. Before this, a create answered with its body first, so a markdown
    /// body of a few KB took the id past the plugin's cut — the first
    /// assertion is that failure, kept so the reason for `slim` stays visible.
    #[test]
    fn every_agent_writer_lands_its_document_chip_through_the_wire_and_a_read_lands_none() {
        let href = format!("/artifacts?a={ID}");
        let long = markdown(4_739);
        let args = r#"{"title": "Launch brief", "markdown": "…"}"#;

        // The old create answer: the whole artifact, body before the end.
        let doc = artifact("doc", "Launch brief", &long);
        assert!(
            door_chips("create_document", args, &json!({ "artifact": doc })).is_empty(),
            "a body-first answer loses its id to the cut"
        );

        let rows: Vec<Vec<String>> = (0..500)
            .map(|r| (0..4).map(|c| format!("r{r}c{c}")).collect())
            .collect();
        let grid = serde_json::to_string(&rows).expect("plain data");
        let html = format!(
            "<html><body>{}</body></html>",
            "<p>a paragraph</p>".repeat(400)
        );
        for (tool, kind, body) in [
            ("create_document", "doc", long.as_str()),
            ("create_sheet", "sheet", grid.as_str()),
            ("create_page", "microsite", html.as_str()),
        ] {
            let a = artifact(kind, "Launch brief", body);
            let chips = door_chips(tool, args, &json!({ "artifact": slim(&a) }));
            assert_eq!(chips.len(), 1, "{tool}: {chips:?}");
            assert_eq!(chips[0]["href"], href, "{tool}");
            assert_eq!(chips[0]["title"], "Launch brief", "{tool}");
        }
        let sheet = artifact("sheet", "Launch tracker", &grid);
        let chips = door_chips(
            "update_document",
            &format!(r#"{{"documentId": "{ID}", "rows": [["a"]]}}"#),
            &json!({ "artifact": slim(&sheet) }),
        );
        assert_eq!(chips.len(), 1, "update_document: {chips:?}");
        assert_eq!(chips[0]["href"], href);

        // A read of the same document — its answer is the whole artifact —
        // lands nothing: reading is for the agent's own context.
        for body in ["a short body", long.as_str()] {
            let read = artifact("doc", "Launch brief", body);
            let chips = door_chips(
                "get_document",
                &format!(r#"{{"documentId": "{ID}"}}"#),
                &json!({ "artifact": read, "editors": editors_json(&agent_grant()) }),
            );
            assert!(chips.is_empty(), "get_document: {chips:?}");
        }
    }

    #[test]
    fn a_base_that_is_not_a_timestamp_is_a_400_sentence() {
        assert_eq!(
            valid_base(Some("2026-11-03T10:00:00.000Z".into()))
                .unwrap()
                .as_deref(),
            Some("2026-11-03T10:00:00.000Z")
        );
        assert_eq!(valid_base(None).unwrap(), None);
        assert!(valid_base(Some("yesterday".into())).is_err());
    }

    #[tokio::test]
    async fn transient_read_names_the_recovery_and_hides_the_engine_error() {
        let res = read_failed(
            "b0bf7d8f-3820-402d-a9e6-8f305607d616",
            "grants read failed",
            "pool timed out waiting for connection",
        );
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            res.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(
            text,
            "{\"error\":\"artifact exists, read failed — retryable. Recover the body with list_documents.\"}"
        );
        assert!(!text.contains("pool timed out"));
    }
}

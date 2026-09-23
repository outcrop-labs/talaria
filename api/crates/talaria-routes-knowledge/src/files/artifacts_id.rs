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
use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_api_facades::kb::perms::{
    ITEM_ARTIFACT, can_edit_agent, can_edit_human, can_govern, can_read, can_read_agent,
    list_editors, set_editors,
};
use talaria_api_facades::retrieval::artifact_routing::apply_artifact_routing;
use talaria_api_facades::retrieval::{embed, qdrant};
use talaria_artifacts::{
    SaveArtifactPatch, delete_artifact, get_artifact, guarded, index_plan_doc, save_artifact,
    set_artifact_official, set_artifact_routing, targets_for_artifact,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    optional_boolean_member, optional_enum_member, optional_max_string_member, parse,
    present_nullable_max_string_member, present_nullable_uuid_member,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user, who_of};
use talaria_state::AppState;
use talaria_users::is_elevated_assistant;

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
    })
}

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
        let owner = match talaria_users::assistant_owner_for(
            &state.pg,
            &AgentSubject::Caller(reader.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(read_failed(&artifact.id, "owner resolve failed", e)),
        };
        let team_ids = match talaria_teams::team_ids_for_agent(&state.pg, &reader.model).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(read_failed(
                    &artifact.id,
                    "team membership read failed (agent)",
                    e,
                ));
            }
        };
        if !can_read_agent(
            &guarded(&artifact),
            &reader.model,
            owner.as_deref(),
            &editors,
            &team_ids,
        ) {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
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
    Ok(Json(json!({ "artifact": artifact, "editors": editors_json(&editors) })).into_response())
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
    let g = guarded(&artifact);

    let actor: String;
    let mut owner = false;
    let agent = agent_caller(&state.pg, &headers).await?;
    if let Some(agent) = agent {
        let name = agent.model.clone();
        // Editor grant — or an admin-elevated assistant on any non-private artifact.
        let elevated = artifact.visibility != "private"
            && match is_elevated_assistant(&state.pg, &AgentSubject::Caller(agent)).await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(arm_failed(
                        "[artifacts] elevation read failed",
                        &artifact.id,
                        "elevation",
                        e,
                    ));
                }
            };
        let team_ids = match talaria_teams::team_ids_for_agent(&state.pg, &name).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(arm_failed(
                    "[artifacts] team membership read failed",
                    &artifact.id,
                    "team-membership-agent",
                    e,
                ));
            }
        };
        let may_edit = can_edit_agent(&name, &editors, &team_ids) || elevated;
        if !may_edit {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        actor = name;
        body.visibility = None;
        body.edit_policy = None;
        body.editors = None;
        body.official = None;
        body.rag_routing = None;
    } else {
        let user = require_user(&state, &headers).await?;
        let who = who_of(&user);
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
        if !can_edit_human(&g, Some(&user.id), who.as_deref(), &editors, &team_ids) {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        actor = actor_of(&user);
        // `canGovern`, not `isOwner` — the same rule kb/docs/{id} already
        // uses, and the reason canGovern exists. Attribution stamps a human
        // owner on what an agent makes, but it cannot promise one: an
        // untraceable caller (or output from before attribution) is ownerless,
        // and strict ownership would leave those files orphans whose sharing
        // literally nobody could change. canGovern hands the ownerless to
        // admins and to whoever may use the agent that wrote them, while
        // owned artifacts — agent-made or not — are owner-only, their
        // owners' to govern.
        owner = match can_govern(&state.pg, &g, &user.id, &user.role, who.as_deref()).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(arm_failed(
                    "[artifacts] govern check failed",
                    &artifact.id,
                    "govern",
                    e,
                ));
            }
        };
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
            title: body.title.as_deref().map(Some),
            body: body.body.as_deref(),
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
    // Content edits keep the artifact's retrievable copy current: auto →
    // the plan-doc activity flow; explicit brain → re-index there.
    if body.body.is_some() || body.title.is_some() {
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
                id.clone(),
            );
            tokio::spawn(async move {
                // The find-plan → index chain is detached; its errors are
                // swallowed.
                if let Ok(targets) = targets_for_artifact(&pg, &id).await
                    && let Some((_, plan_id)) = targets.iter().find(|(tt, _)| tt == "plan")
                {
                    let _ = index_plan_doc(&pg, &qd, &ed, &u, plan_id).await;
                }
            });
        }
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

// /api/plans/{id}/doc — port of ui/src/routes/api/plans.$id.doc.ts.
// The plan's living document (a linked doc artifact). GET → find-or-create
// it, seeded from the agent's plan template when one is bound. POST → the
// plan's agent rewrites it from the conversation so far. Owner or plan
// collaborator; the document is always OWNED by the plan's owner regardless
// of who touched it first.

use crate::body::{as_object, nullish_max_string_member, parse};
use crate::conversations::accessible_conversation;
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::{routed_model_for, usable_agent_gate};
use crate::params::uuid_gate;
use crate::plan_doc::{PlanOwner, ensure_plan_doc, sync_plan_doc};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate("plans", "GET doc", &id) {
        return gate;
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[plans] accessible read on GET doc failed: {e}");
            return thrown_internal_error();
        }
    };
    // A chat conversation is reachable through the same helper; only plans
    // have a document.
    let Some(conv) = conv.filter(|c| c.kind == "plan") else {
        return house_error(StatusCode::NOT_FOUND, "plan not found");
    };
    let label = user
        .name
        .clone()
        .or(user.email)
        .unwrap_or_else(|| "someone".into());
    match ensure_plan_doc(
        &state.pg,
        &id,
        PlanOwner {
            id: &conv.owner_user_id,
            label: &label,
        },
        conv.title.as_deref(),
        Some(&conv.agent_model),
        conv.plan_template_id.as_deref(),
    )
    .await
    {
        Ok(artifact) => Json(json!({ "artifact": artifact })).into_response(),
        Err(e) => {
            tracing::error!("[plans] ensure doc failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate("plans", "POST doc", &id) {
        return gate;
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[plans] accessible read on POST doc failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(conv) = conv.filter(|c| c.kind == "plan") else {
        return house_error(StatusCode::NOT_FOUND, "plan not found");
    };
    // The gate checks the plan's OWN agent, so a member who cannot drive this
    // agent cannot spend it rewriting the document either.
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[plans] agent access read on POST doc failed: {e}");
            return thrown_internal_error();
        }
    };
    if !gate(&conv.agent_model) {
        return house_error(
            StatusCode::FORBIDDEN,
            "you do not have access to that agent",
        );
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let tier = match nullish_max_string_member(obj, "tier", 60) {
        Ok(t) => t,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // `body.tier ? routedModelFor(agentModel, tier).catch(() => null) : null`
    // `?? conv.agentModel` — an unknown tier falls back to the base agent
    // rather than refusing the sync.
    let routed = match tier.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => routed_model_for(&state.pg, &conv.agent_model, Some(t))
            .await
            .unwrap_or_default(),
        None => None,
    }
    .unwrap_or_else(|| conv.agent_model.clone());

    let label = user
        .name
        .clone()
        .or(user.email)
        .unwrap_or_else(|| "someone".into());
    match sync_plan_doc(
        &state,
        &id,
        PlanOwner {
            id: &conv.owner_user_id,
            label: &label,
        },
        conv.title.as_deref(),
        &conv.agent_model,
        &routed,
        conv.plan_template_id.as_deref(),
    )
    .await
    {
        Ok(artifact) => Json(json!({ "artifactId": artifact.id })).into_response(),
        // The engine's sentence is the toast: empty document, unreachable
        // agent, or the data-loss guard keeping the existing one.
        Err(msg) => house_error(StatusCode::BAD_GATEWAY, &msg),
    }
}

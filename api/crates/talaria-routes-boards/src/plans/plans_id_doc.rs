// /api/plans/{id}/doc.
// The plan's living document (a linked doc artifact). GET → find-or-create
// it, seeded from the agent's plan template when one is bound. POST → the
// plan's agent rewrites it from the conversation so far. Owner or plan
// collaborator; the document is always OWNED by the plan's owner regardless
// of who touched it first.
//
// A WORK SESSION HAS NO PLAN DOCUMENT. Its pane shows the files the agent
// creates and opens (`GET /api/conversations/{id}/files`), so opening one
// creates nothing here: GET answers the living document only for a legacy
// session that already has one (404 otherwise), and POST — a rewrite of the
// document from the transcript — is a 409, because a work session's files are
// written by the agent's own tools, never re-derived from the chat.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::{routed_model_for, usable_agent_gate};
use talaria_body::{nullish_max_string_member, parse};
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal, object_or_400};
use talaria_params::uuid_gate;
use talaria_plan_doc::{PlanOwner, ensure_plan_doc, plan_doc_for, sync_plan_doc};
use talaria_session::require_user;
use talaria_state::AppState;

/// The 404 a work session without a legacy living document answers.
const NO_PLAN_DOCUMENT: &str = "this session has no plan document";

/// The 409 a sync on a work session answers.
const WORK_FILES_ARE_WRITTEN_BY_THE_AGENT: &str =
    "a work session's files are written by the agent; there is no plan document to sync";

// doc: A plan: its living document, created (from the plan template) on the
// doc: first read. A work session: the living document a legacy session
// doc: already has, else 404 "this session has no plan document" — never
// doc: creates one.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = uuid_gate("plans", "GET doc", &id) {
        return Ok(gate);
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[plans] accessible read on GET doc failed", e)),
    };
    // A chat conversation is reachable through the same helper; the surface
    // with a living document is a plan. A work session answers here only for
    // the document a legacy session already has.
    let Some(conv) = conv.filter(|c| c.kind == "plan" || c.kind == "work") else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    if conv.kind == "work" {
        // NEVER CREATES. Opening a work session used to mint a "Plan — …"
        // document nobody asked for; the pane now lists the session's files.
        return Ok(match plan_doc_for(&state.pg, &id).await {
            Ok(Some(artifact)) => Json(json!({ "artifact": artifact })).into_response(),
            Ok(None) => house_error(StatusCode::NOT_FOUND, NO_PLAN_DOCUMENT),
            Err(e) => internal("[plans] living doc read on GET doc failed", e),
        });
    }
    let label = user
        .name
        .clone()
        .or(user.email)
        .unwrap_or_else(|| "someone".into());
    Ok(
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
            Err(e) => internal("[plans] ensure doc failed", e),
        },
    )
}

// doc: A plan: the plan's agent rewrites the document from the conversation
// doc: so far. A work session: 409 — its files are written by the agent's own
// doc: tools, and there is no plan document to sync.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = uuid_gate("plans", "POST doc", &id) {
        return Ok(gate);
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[plans] accessible read on POST doc failed", e)),
    };
    let Some(conv) = conv.filter(|c| c.kind == "plan" || c.kind == "work") else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    // A work session is found (it is not a 404) but has nothing to sync: its
    // files are the agent's own writes, and a rewrite from the transcript
    // would overwrite them with a summary of the chat.
    if conv.kind == "work" {
        return Ok(house_error(
            StatusCode::CONFLICT,
            WORK_FILES_ARE_WRITTEN_BY_THE_AGENT,
        ));
    }
    // The gate checks the plan's OWN agent, so a member who cannot drive this
    // agent cannot spend it rewriting the document either.
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => return Ok(internal("[plans] agent access read on POST doc failed", e)),
    };
    if !gate(&conv.agent_model) {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "you do not have access to that agent",
        ));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let tier = match nullish_max_string_member(obj, "tier", 60) {
        Ok(t) => t,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // an unknown tier falls back to the base agent rather than refusing
    // the sync.
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
    Ok(
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
            &conv.kind,
        )
        .await
        {
            Ok(artifact) => Json(json!({ "artifactId": artifact.id })).into_response(),
            // The engine's sentence is the toast: empty document, unreachable
            // agent, or the data-loss guard keeping the existing one.
            Err(msg) => house_error(StatusCode::BAD_GATEWAY, &msg),
        },
    )
}

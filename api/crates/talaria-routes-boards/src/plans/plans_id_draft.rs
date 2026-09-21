// /api/plans/{id}/draft.
// The plan surface's ticket drafts as a DURABLE JOB: POST enqueues a
// 'plan-draft' run on the CONVERSATION'S OWN agent (the body names no agent
// — that is the one difference from the channel twin) and answers with the
// queued draft; GET is the review's way back to the conversation's latest
// draft; PATCH persists the walk's edits; DELETE drops a consumed or
// discarded batch. Nothing is created by any of this — the human reviews and
// creates via the boards API (PlanModal).
//
// The gate is the conversation-ownership rule (conversations.rs): the owner,
// or a collaborator of a plan. The 404 body says 'plan not found' for both a
// missing conversation and an inaccessible one — never leaking which.
// The ownership rule now also admits RESEARCH conversations (their access
// follows the run), which this surface must refuse: a research discussion is
// not a plan, so `draftable_conversation` re-narrows to the kinds that have
// always drafted here — plans, and the chats the rule has always admitted.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::PgPool;
use talaria_api_facades::fleet::{routed_model_for, usable_agent_gate};
use talaria_body::parse;
use talaria_conversations::{accessible_conversation_agent, conversation_accessible};
use talaria_error::{house_error, internal, object_or_400};
use talaria_params::uuid_gate;
use talaria_plan_drafts::{
    StartPlanDraft, drop_draft, latest_draft_for, save_draft_proposals, start_plan_draft,
};
use talaria_routes_comms::comms::channels_id_plan::{validate_save_body, validate_start};
use talaria_session::require_user;
use talaria_state::AppState;

async fn draftable_conversation(pg: &PgPool, id: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("select kind in ('chat', 'plan') from conversations where id = $1::uuid")
            .bind(id)
            .fetch_optional(pg)
            .await?;
    Ok(row.map(|(draftable,)| draftable).unwrap_or(false))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // a non-uuid id answers the 500 here — it never reaches the uuid bind.
    if let Some(gate) = uuid_gate("plans", "GET draft", &id) {
        return Ok(gate);
    }
    match draftable_conversation(&state.pg, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "plan not found")),
        Err(e) => return Ok(internal("[plans] kind read on GET draft failed", e)),
    }
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "plan not found")),
        Err(e) => return Ok(internal("[plans] accessible read on GET draft failed", e)),
    }
    Ok(match latest_draft_for(&state.pg, &id).await {
        Ok(draft) => Json(json!({ "draft": draft })).into_response(),
        Err(e) => internal("[plans] draft read failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = uuid_gate("plans", "POST draft", &id) {
        return Ok(gate);
    }
    match draftable_conversation(&state.pg, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "plan not found")),
        Err(e) => return Ok(internal("[plans] kind read on POST draft failed", e)),
    }
    // The conversation's own agent drafts — the body cannot name one. None
    // covers "not accessible" and "no agent" both, and the 404 never says
    // which (see accessible_conversation_agent).
    let agent_model = match accessible_conversation_agent(&state.pg, &user.id, &id).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "plan not found")),
        Err(e) => return Ok(internal("[plans] accessible read on POST draft failed", e)),
    };
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            return Ok(internal(
                "[plans] agent access read on POST draft failed",
                e,
            ));
        }
    };
    if !gate(&agent_model) {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "you do not have access to that agent",
        ));
    }
    // The agent checks precede the body on this route (the channel twin
    // parses first): a member with a bad body on a plan they cannot draft
    // gets the 403/404, not the 400.
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let start = match validate_start(obj, false) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Same routing posture as the channel twin: a blank tier never asks, a
    // failed read or unknown tier falls back to the base agent.
    let routed = match start.tier.as_deref() {
        Some(t) if !t.is_empty() => routed_model_for(&state.pg, &agent_model, Some(t))
            .await
            .ok()
            .flatten(),
        _ => None,
    }
    .unwrap_or_else(|| agent_model.clone());
    Ok(
        match start_plan_draft(
            &state,
            StartPlanDraft {
                conversation_id: &id,
                source: "plan",
                user_id: &user.id,
                agent_model: &agent_model,
                routed_model: &routed,
                tier: start.tier.as_deref(),
                board_id: start.board_id.as_deref(),
                template_id: start.template_id.as_deref(),
            },
        )
        .await
        {
            Ok(draft) => Json(json!({ "draft": draft })).into_response(),
            Err(e) => {
                // The row-creation failure is never a sentence worth showing
                // (docker text, pg internals); the run's OWN failures reach the
                // client as the draft row's `error` field, not through this 500.
                tracing::error!(
                    "[plans] start plan draft failed for conversation {id} agent {agent_model}: {e}"
                );
                house_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not start the plan draft — see server logs",
                )
            }
        },
    )
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = uuid_gate("plans", "PATCH draft", &id) {
        return Ok(gate);
    }
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "plan not found")),
        Err(e) => return Ok(internal("[plans] accessible read on PATCH draft failed", e)),
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let proposals = match validate_save_body(obj) {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = save_draft_proposals(&state.pg, &id, &proposals).await {
        return Ok(internal("[plans] save draft proposals failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = uuid_gate("plans", "DELETE draft", &id) {
        return Ok(gate);
    }
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "plan not found")),
        Err(e) => {
            return Ok(internal(
                "[plans] accessible read on DELETE draft failed",
                e,
            ));
        }
    }
    if let Err(e) = drop_draft(&state, &id).await {
        return Ok(internal("[plans] drop draft failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

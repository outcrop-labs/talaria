// /api/research — port of ui/src/routes/api/research.ts.
// GET → recent runs scoped to the viewer + the mode catalog. POST { question,
// mode?, agentModel? } → start a run. Humans and agents both start research;
// an agent researches AS ITSELF, pinned to its own model.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::body::{as_object, optional_string_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::usable_agent_gate;
use crate::harness::defs::research::ResearchDepth;
use crate::permissions::has_perm;
use crate::research::{self, ResearchRun, research_run_for_conversation, start_research};
use crate::research_origin::{current_agent_turn, remember_research_origin};
use crate::runs::defs::research::research_modes;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// `z.enum(['recon','brief','expedition']).default('brief')` — absent is the
/// default mode, a present value must be one of the three.
fn mode_member(obj: &serde_json::Map<String, serde_json::Value>) -> Result<ResearchDepth, String> {
    let mode = match obj.get("mode") {
        None => "brief".to_string(),
        Some(_) => crate::body::enum_member(obj, "mode", &["recon", "brief", "expedition"])?,
    };
    Ok(match mode.as_str() {
        "recon" => ResearchDepth::Recon,
        "expedition" => ResearchDepth::Expedition,
        _ => ResearchDepth::Brief,
    })
}

/// The mode catalog the picker renders: [{mode, blurb}].
fn modes_json() -> serde_json::Value {
    json!(
        research_modes()
            .into_iter()
            .map(|(mode, blurb)| json!({ "mode": mode, "blurb": blurb }))
            .collect::<Vec<_>>()
    )
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Scope to the viewer: a user sees their own + shared + org runs; an
    // agent sees through its owner's eyes (general agents: org runs only).
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return get_as_user(&state, &headers).await,
        Err(resp) => return resp,
    };
    // The CALLER: seeing a human's private runs is owner-proxying, so an
    // asserted identity resolves to no owner (org runs only).
    let viewer =
        match crate::users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller)).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[research] owner resolve on list failed: {e}");
                return thrown_internal_error();
            }
        };
    match research::list_research_runs(&state.pg, viewer.as_deref(), 60).await {
        Ok(runs) => Json(json!({ "runs": runs, "modes": modes_json() })).into_response(),
        Err(e) => {
            tracing::error!("[research] list failed: {e}");
            thrown_internal_error()
        }
    }
}

async fn get_as_user(state: &AppState, headers: &HeaderMap) -> Response {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match research::list_research_runs(&state.pg, Some(&user.id), 60).await {
        Ok(runs) => Json(json!({ "runs": runs, "modes": modes_json() })).into_response(),
        Err(e) => {
            tracing::error!("[research] list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let question = match string_member(obj, "question", 8, 4000) {
        Ok(q) => q,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let mode = match mode_member(obj) {
        Ok(m) => m,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body_agent = match optional_string_member(obj, "agentModel", 200) {
        Ok(a) => a,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => Some(c),
        Ok(None) => None,
        Err(resp) => return resp,
    };
    let (agent_model, owner_user_id, requested_by) = if let Some(caller) = caller.as_ref() {
        // An agent researches in its own field; its owner (for a personal
        // assistant) is the one the notification reaches.
        let owner = match crate::users::assistant_owner_for(
            &state.pg,
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[research] owner resolve on start failed: {e}");
                return thrown_internal_error();
            }
        };
        (caller.model.clone(), owner, caller.model.clone())
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        match has_perm(&state.pg, &user.id, &user.role, "research.run").await {
            Ok(true) => {}
            Ok(false) => {
                return house_error(StatusCode::FORBIDDEN, "no permission to run research");
            }
            Err(e) => {
                tracing::error!("[research] perm read on start failed: {e}");
                return thrown_internal_error();
            }
        }
        // Humans pick the agent (and need access to it); an agent-key caller
        // is pinned to itself above and never reaches this leg.
        let Some(agent_model) = body_agent else {
            return house_error(StatusCode::BAD_REQUEST, "agentModel required");
        };
        let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("[research] agent access read on start failed: {e}");
                return thrown_internal_error();
            }
        };
        if !gate(&agent_model) {
            return house_error(StatusCode::FORBIDDEN, "forbidden: no access to this agent");
        }
        (
            agent_model,
            Some(user.id.clone()),
            user.email
                .clone()
                .or(user.name.clone())
                .unwrap_or_else(|| "user".into()),
        )
    };

    // A queued/running duplicate of the same question is a double-click. TS's
    // own spelling, raw column and all — the projection's run-aware CASE is
    // `active_research_on`'s, and the route never called it.
    let dupe: Option<(String,)> = match sqlx::query_as(
        "select id::text from research_runs \
         where question = $1 and status in ('queued', 'running') limit 1",
    )
    .bind(&question)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[research] duplicate check failed: {e}");
            return thrown_internal_error();
        }
    };
    if let Some((id,)) = dupe {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "run": null, "duplicateOf": id })),
        )
            .into_response();
    }

    // ── IS THIS A FOLLOW-UP? ──────────────────────────────────────────────────
    //
    // INFERRED, NEVER ASKED FOR. An agent commissioning research from inside a
    // research conversation is extending THAT report, and it already knows
    // which one — the conversation it is speaking in belongs to a run. So the
    // parent is looked up here rather than added as a tool parameter for a
    // model to fill in, which means a model cannot get it wrong, pass a stale
    // runId, or forget. The best fix for a mistake is to make it unavailable.
    //
    // Agent callers only: a person on the Research page who types a new
    // question is starting a new subject, and the page they are looking at
    // has its own "ask a follow-up" affordance for the other case.
    let origin = if caller.is_some() {
        current_agent_turn(&state, &agent_model).await
    } else {
        None
    };
    let parent_run_id = match &origin {
        Some(conversation) => research_run_for_conversation(&state.pg, conversation)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    let run: ResearchRun = match start_research(
        &state,
        crate::runs::defs::research::ResearchInput {
            question: question.clone(),
            mode,
            agent_model: agent_model.clone(),
            owner_user_id,
            requested_by,
            parent_run_id: parent_run_id.clone(),
        },
    )
    .await
    {
        Ok(r) => r,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // WHO IS OWED THE ANSWER. An agent that starts a run mid-conversation
    // cannot wait for it — the tool returns a runId and the turn ends — so the
    // run remembers the chat it came out of and reports back there when it
    // finishes. Only for agent callers: a human who started a run from the
    // Research page is already looking at the page that updates.
    if caller.is_some()
        && let Some(origin) = &origin
    {
        remember_research_origin(&state, &run.id, origin).await;
    }
    Json(json!({ "run": run })).into_response()
}

// /api/admin/platform-agents — port of ui/src/routes/api/admin.platform-agents.ts.
// Platform sub-agents — Talaria's own workers — and which model powers each.
// GET → registry + assignments + assignable models. PUT { id, model|null } →
// assign (null = back to auto). The Judge's pick lives in its own
// judge_config (shared with the Guard panel) — this route reads/writes it
// there so there's one source of truth. Admins only.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, enum_member, nullish_member, optional_max_string_member, parse, string_msg,
    too_big_msg, too_small_msg, utf16_len, zod_type_name,
};
use crate::effort_prefs::{agent_slot, get_effort_prefs, set_effort_pref};
use crate::error::{house_error, thrown_internal_error};
use crate::judge::{get_judge_config, set_judge_config};
use crate::model_access::gateway_models;
use crate::model_efforts::efforts_for_model;
use crate::platform_agents::{
    PLATFORM_AGENTS, get_platform_agent_models, set_platform_agent_model,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// The assignable ids in catalog order — z.enum's set, and the order the
/// `efforts` map builds in.
fn assignable_ids() -> Vec<&'static str> {
    PLATFORM_AGENTS
        .iter()
        .filter(|a| a.assignable)
        .map(|a| a.id)
        .collect()
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    // assignments = the raw setting, with judge overlaid from judge_config —
    // an undefined judge model DROPS the key (undefined never serializes).
    let mut assignments = match get_platform_agent_models(&state.pg).await {
        Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    let judge_model = get_judge_config(&state.pg).await;
    match judge_model.get("model") {
        Some(Value::String(m)) => {
            assignments.insert("judge".into(), Value::String(m.clone()));
        }
        _ => {
            assignments.remove("judge");
        }
    }
    // (await gatewayModels()).map — no catch: a gateway read failure is a 500
    // here, unlike the judge panel's graceful [].
    let models: Vec<String> = match gateway_models(&state.pg).await {
        Ok(m) => m.into_iter().map(|m| m.id).collect(),
        Err(e) => {
            tracing::error!("[admin/platform-agents] gateway read failed: {e}");
            return thrown_internal_error();
        }
    };
    let prefs = get_effort_prefs(&state.pg).await;
    let mut efforts = serde_json::Map::new();
    for id in assignable_ids() {
        let slot = agent_slot(id);
        let effort = prefs.get(&slot).cloned().unwrap_or(Value::Null); // ?? null
        efforts.insert(id.to_string(), effort);
    }
    Json(serde_json::json!({
        "agents": PLATFORM_AGENTS.iter().map(|a| a.to_json()).collect::<Vec<_>>(),
        "assignments": Value::Object(assignments),
        "models": models,
        // The per-agent effort preference (null = the model's own default).
        "efforts": Value::Object(efforts),
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.object({ id: z.enum(IDS), model: z.string().max(200).nullish(),
    //             effort: z.string().min(1).max(24).nullish() }) — the enum's
    // message lists every assignable id in catalog order.
    let id = match enum_member(obj, "id", &assignable_ids()) {
        Ok(i) => i,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Tri-state: absent leaves the assignment alone, null clears it, a
    // string sets it — the PATCH distinction nullish_member exists for.
    let model = match nullish_member(obj, "model", |o, k| optional_max_string_member(o, k, 200)) {
        Ok(m) => m,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Absent = leave it alone; null = clear.
    let effort = match obj.get("effort") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(e)) => {
            let n = utf16_len(e);
            if n < 1 {
                return house_error(StatusCode::BAD_REQUEST, &too_small_msg(1));
            }
            if n > 24 {
                return house_error(StatusCode::BAD_REQUEST, &too_big_msg(24));
            }
            Some(Some(e.clone()))
        }
        Some(v) => return house_error(StatusCode::BAD_REQUEST, &string_msg(zod_type_name(v))),
    };

    // `if (body.model && ...)` — the truthy spelling: an empty string skips
    // the gateway check entirely (and later writes as a clear).
    if let Some(Some(m)) = &model {
        if m.is_empty() {
            // falsy in TS — no membership check
        } else {
            let on_gateway = match gateway_models(&state.pg).await {
                Ok(models) => models.iter().any(|g| &g.id == m),
                Err(e) => {
                    tracing::error!("[admin/platform-agents] gateway read failed: {e}");
                    return thrown_internal_error();
                }
            };
            if !on_gateway {
                return house_error(StatusCode::BAD_REQUEST, "that model is not on the gateway");
            }
        }
    }

    if let Some(effort) = &effort {
        // Same rule as the roles route: validated against the assigned
        // model's published levels, and refused on Auto where there is
        // nothing to ride.
        let current = if id == "judge" {
            get_judge_config(&state.pg)
                .await
                .get("model")
                .and_then(Value::as_str)
                .map(String::from)
        } else {
            get_platform_agent_models(&state.pg)
                .await
                .get(id.as_str())
                .and_then(Value::as_str)
                .map(String::from)
        };
        let target = match &model {
            Some(Some(m)) => Some(m.clone()),
            _ => current,
        };
        if let Some(e) = effort.as_deref() {
            let Some(target) = target else {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "assign a model before setting its effort",
                );
            };
            if !efforts_for_model(&state.pg, &target)
                .await
                .iter()
                .any(|lvl| lvl == e)
            {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &format!("that model does not publish the \"{e}\" effort level"),
                );
            }
        }
        if let Err(e) = set_effort_pref(&state.pg, &agent_slot(&id), effort.as_deref()).await {
            tracing::error!("[admin/platform-agents] effort write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "platform_agent.effort",
                target_type: "platform-agent",
                target_id: Some(&id),
                target_label: None,
                before: None,
                after: Some({
                    // { effort } — undefined drops, null serializes.
                    let mut m = serde_json::Map::new();
                    m.insert(
                        "effort".into(),
                        effort.clone().map(Value::String).unwrap_or(Value::Null),
                    );
                    Value::Object(m)
                }),
            },
        )
        .await;
    }

    // TS writes model: undefined as `model !== undefined` gates the branch —
    // absent never reaches either store.
    if let Some(model) = &model {
        if id == "judge" {
            let mut cfg = get_judge_config(&state.pg).await;
            if let Some(o) = cfg.as_object_mut() {
                o.insert(
                    "model".into(),
                    model.clone().map(Value::String).unwrap_or(Value::Null),
                );
            }
            set_judge_config(&state.pg, &cfg).await;
        } else if let Err(e) = set_platform_agent_model(&state.pg, &id, model.as_deref()).await {
            tracing::error!("[admin/platform-agents] assign write failed: {e}");
            return thrown_internal_error();
        }
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "platform_agent.assign",
            target_type: "platform-agent",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some({
                let mut m = serde_json::Map::new();
                if let Some(model) = &model {
                    m.insert(
                        "model".into(),
                        model.clone().map(Value::String).unwrap_or(Value::Null),
                    );
                }
                Value::Object(m)
            }),
        },
    )
    .await;
    Json(serde_json::json!({ "ok": true })).into_response()
}

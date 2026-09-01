// /api/admin/outreach — port of ui/src/routes/api/admin.outreach.ts.
// GET → config + per-agent proactive flags + recent events. PUT → save both.
// Admin-only; the sweep itself stays off unless `enabled`.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{NumKind, array_too_big_msg, as_object, boolean_member, number_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::outreach::{
    OutreachConfig, get_outreach_config, recent_outreach_events, set_outreach_config,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let agents: Result<Vec<(String, String, bool, bool)>, sqlx::Error> = sqlx::query_as(
        "select model, display_name, proactive, owner_user_id is not null \
         from agent_defs where enabled order by slug",
    )
    .fetch_all(&state.pg)
    .await;
    let agents = match agents {
        Ok(a) => a
            .into_iter()
            .map(|(model, display_name, proactive, personal)| {
                serde_json::json!({
                    "model": model,
                    "displayName": display_name,
                    "proactive": proactive,
                    "personal": personal,
                })
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            tracing::error!("[admin/outreach] agents read failed: {e}");
            return thrown_internal_error();
        }
    };
    let c = get_outreach_config(&state.pg).await;
    Json(serde_json::json!({
        "config": {
            "enabled": c.enabled,
            "intervalMinutes": c.interval_minutes,
            "dailyDmCap": c.daily_dm_cap,
        },
        "agents": agents,
        "events": recent_outreach_events(&state.pg, 30).await,
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
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.object({ enabled: z.boolean(),
    //             intervalMinutes: z.number().int().min(15).max(1440),
    //             dailyDmCap: z.number().int().min(1).max(20),
    //             proactiveAgents: z.array(z.string()).max(100) }) — keys in
    // schema order, rejections in zod's own words.
    let enabled = match boolean_member(obj, "enabled") {
        Ok(e) => e,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let interval = match number_member(obj, "intervalMinutes", NumKind::Int, 15.0, 1440.0) {
        Ok(n) => n as i64,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let cap = match number_member(obj, "dailyDmCap", NumKind::Int, 1.0, 20.0) {
        Ok(n) => n as i64,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let proactive = match obj.get("proactiveAgents") {
        None => {
            return house_error(
                StatusCode::BAD_REQUEST,
                &crate::body::array_msg("undefined"),
            );
        }
        Some(v) => match v.as_array() {
            Some(a) => {
                if a.len() > 100 {
                    return house_error(StatusCode::BAD_REQUEST, &array_too_big_msg(100));
                }
                let mut out = Vec::with_capacity(a.len());
                for x in a {
                    let s = x
                        .as_str()
                        .ok_or_else(|| crate::body::string_msg(crate::body::zod_type_name(x)));
                    match s {
                        Ok(s) => out.push(s.to_string()),
                        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                    }
                }
                out
            }
            None => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &crate::body::array_msg(crate::body::zod_type_name(v)),
                );
            }
        },
    };

    set_outreach_config(
        &state.pg,
        &OutreachConfig {
            enabled,
            interval_minutes: interval,
            daily_dm_cap: cap,
        },
    )
    .await;
    // One statement for the whole fleet: proactive is exactly "in the list".
    if let Err(e) = sqlx::query("update agent_defs set proactive = (model = any($1)) where enabled")
        .bind(&proactive)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[admin/outreach] flags write failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "outreach.config",
            target_type: "outreach",
            target_id: None,
            target_label: None,
            before: None,
            after: Some(serde_json::json!({ "proactiveAgents": proactive })),
        },
    )
    .await;
    Json(serde_json::json!({ "ok": true })).into_response()
}

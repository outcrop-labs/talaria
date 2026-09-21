// /api/admin/outreach. GET → config + per-agent proactive flags + recent
// events. PUT → save both. Admin-only; the sweep itself stays off unless
// `enabled`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{NumKind, array_too_big_msg, boolean_member, number_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_outreach::{
    OutreachConfig, get_outreach_config, recent_outreach_events, set_outreach_config,
};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
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
        Err(e) => return Ok(internal("[admin/outreach] agents read failed", e)),
    };
    let c = get_outreach_config(&state.pg).await;
    Ok(Json(serde_json::json!({
        "config": {
            "enabled": c.enabled,
            "intervalMinutes": c.interval_minutes,
            "dailyDmCap": c.daily_dm_cap,
        },
        "agents": agents,
        "events": recent_outreach_events(&state.pg, 30).await,
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Keys in schema order, rejections in the schema's own words: enabled
    // (bool), intervalMinutes (int 15..1440), dailyDmCap (int 1..20),
    // proactiveAgents (strings, at most 100).
    let enabled = match boolean_member(obj, "enabled") {
        Ok(e) => e,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let interval = match number_member(obj, "intervalMinutes", NumKind::Int, 15.0, 1440.0) {
        Ok(n) => n as i64,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let cap = match number_member(obj, "dailyDmCap", NumKind::Int, 1.0, 20.0) {
        Ok(n) => n as i64,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let proactive = match obj.get("proactiveAgents") {
        None => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::array_msg("undefined"),
            ));
        }
        Some(v) => match v.as_array() {
            Some(a) => {
                if a.len() > 100 {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &array_too_big_msg(100),
                    ));
                }
                let mut out = Vec::with_capacity(a.len());
                for x in a {
                    let s = x
                        .as_str()
                        .ok_or_else(|| talaria_body::string_msg(talaria_body::zod_type_name(x)));
                    match s {
                        Ok(s) => out.push(s.to_string()),
                        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
                    }
                }
                out
            }
            None => {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    &talaria_body::array_msg(talaria_body::zod_type_name(v)),
                ));
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
        return Ok(internal("[admin/outreach] flags write failed", e));
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
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

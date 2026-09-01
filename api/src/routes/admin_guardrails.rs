// /api/admin/guardrails — port of ui/src/routes/api/admin.guardrails.ts.
// Confab guardrail config + observability (admin). GET → config + stats +
// recent findings. PUT → update config. The config is stored RAW (the
// five-key zod shape) and the GET is the spread read: defaults under stored,
// numbers passing through as written — never re-serialized from an f64.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, array_msg, array_too_big_msg, as_object, boolean_member, boolean_msg, enum_member,
    number_member, parse, record_msg, string_msg, too_big_msg, utf16_len, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::gateway::settings::{get_setting, set_setting};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

const CONFIG_KEY: &str = "guardrails_config";

/// getGuardConfig's `{...DEFAULT_CONFIG, ...stored}` for the WIRE: the five
/// schema keys in DEFAULT_CONFIG's order, each falling to its default when
/// the stored object lacks it. Values are passed through verbatim (a stored
/// `minConfidence: 1` reads back as 1, not 1.0).
async fn config_for_wire(pg: &sqlx::PgPool) -> Value {
    let stored = get_setting(pg, CONFIG_KEY, serde_json::json!({})).await;
    let mut out = serde_json::Map::new();
    let empty = serde_json::Map::new();
    let s = stored.as_object().unwrap_or(&empty);
    out.insert(
        "mode".into(),
        s.get("mode")
            .cloned()
            .unwrap_or(serde_json::json!("observe")),
    );
    out.insert(
        "checks".into(),
        s.get("checks").cloned().unwrap_or(serde_json::json!({})),
    );
    out.insert(
        "minConfidence".into(),
        s.get("minConfidence")
            .cloned()
            .unwrap_or(serde_json::json!(0.5)),
    );
    out.insert(
        "policedHosts".into(),
        s.get("policedHosts")
            .cloned()
            .unwrap_or(serde_json::json!([])),
    );
    out.insert(
        "coach".into(),
        s.get("coach").cloned().unwrap_or(serde_json::json!(false)),
    );
    Value::Object(out)
}

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let config = config_for_wire(&state.pg).await;
    let stats = crate::gateway::guard::guard_stats(&state.pg).await;
    // listFindings throws to the 500 boundary in TS; a read failure is a
    // failure, not an empty list.
    let findings = match crate::gateway::guard::list_guard_findings(&state.pg, 50).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[admin/guardrails] findings read failed: {e}");
            return thrown_internal_error();
        }
    };
    let rules = crate::gateway::guard::guard_rule_meta();
    Json(serde_json::json!({
        "config": config,
        "stats": stats,
        "findings": findings,
        "rules": rules,
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
    // Keys in schema order, every rejection in zod's own words:
    //   mode: z.enum(['off','observe','annotate','strict'])
    //   checks: z.record(z.string(), z.boolean())
    //   minConfidence: z.number().min(0).max(1)
    //   policedHosts: z.array(z.string().max(200)).max(100)
    //   coach: z.boolean().default(false)
    let mode = match enum_member(obj, "mode", &["off", "observe", "annotate", "strict"]) {
        Ok(m) => m,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let checks = match obj.get("checks") {
        None => return house_error(StatusCode::BAD_REQUEST, &record_msg("undefined")),
        Some(v) => match v.as_object() {
            Some(o) => {
                if let Some(bad) = o.values().find(|v| !v.is_boolean()) {
                    return house_error(StatusCode::BAD_REQUEST, &boolean_msg(zod_type_name(bad)));
                }
                o.clone()
            }
            None => return house_error(StatusCode::BAD_REQUEST, &record_msg(zod_type_name(v))),
        },
    };
    // Validated only — the stored config passes the number through as the
    // client wrote it (a stored 1 reads back as 1, not 1.0).
    if let Err(msg) = number_member(obj, "minConfidence", NumKind::Float, 0.0, 1.0) {
        return house_error(StatusCode::BAD_REQUEST, &msg);
    }
    let hosts = match obj.get("policedHosts") {
        None => return house_error(StatusCode::BAD_REQUEST, &array_msg("undefined")),
        Some(v) => match v.as_array() {
            Some(a) => {
                if a.len() > 100 {
                    return house_error(StatusCode::BAD_REQUEST, &array_too_big_msg(100));
                }
                for h in a {
                    let Some(s) = h.as_str() else {
                        return house_error(StatusCode::BAD_REQUEST, &string_msg(zod_type_name(h)));
                    };
                    if utf16_len(s) > 200 {
                        return house_error(StatusCode::BAD_REQUEST, &too_big_msg(200));
                    }
                }
                a.clone()
            }
            None => return house_error(StatusCode::BAD_REQUEST, &array_msg(zod_type_name(v))),
        },
    };
    let coach = match obj.get("coach") {
        None => false, // .default(false)
        Some(_) => match boolean_member(obj, "coach") {
            Ok(b) => b,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        },
    };
    // The stored object is the parsed body in schema order — mode, checks,
    // minConfidence, policedHosts, coach — with the numbers carried through
    // as the client wrote them.
    let mut stored = serde_json::Map::new();
    stored.insert("mode".into(), serde_json::json!(mode));
    stored.insert("checks".into(), Value::Object(checks.clone()));
    stored.insert(
        "minConfidence".into(),
        obj.get("minConfidence").cloned().unwrap(),
    );
    stored.insert("policedHosts".into(), Value::Array(hosts.clone()));
    stored.insert("coach".into(), serde_json::json!(coach));
    let stored = Value::Object(stored);
    if let Err(e) = set_setting(&state.pg, CONFIG_KEY, &stored).await {
        tracing::error!("[admin/guardrails] config write failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "settings.guardrails",
            target_type: "settings",
            target_id: None,
            target_label: None,
            before: None,
            after: Some(serde_json::json!({ "mode": mode })),
        },
    )
    .await;
    Json(serde_json::json!({ "config": stored })).into_response()
}

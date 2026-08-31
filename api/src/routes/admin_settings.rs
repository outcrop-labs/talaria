// /api/admin/settings — port of ui/src/routes/api/admin.settings.ts. App
// settings (admin). GET → current values. PUT → update. Grows as more
// app-wide settings land; audit retention is the first.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, utf16_len, zod_type_name};
use crate::error::{house_error, thrown_internal_error};
use crate::gateway::settings::{get_setting, set_setting};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let org = crate::org::org_profile(&state.pg).await;
    Json(serde_json::json!({
        "auditRetentionDays": audit_retention_days(&state.pg).await,
        "org": { "name": org.name, "about": org.about },
        "memberModels": crate::model_access::member_model_allowlist(&state.pg).await,
        "llmBudgets": get_setting(
            &state.pg,
            "llm_budgets",
            serde_json::json!({"windowHours": 24, "org": null, "perAgent": null, "agents": {}}),
        )
        .await,
        "cronMinIntervalMinutes": get_setting(&state.pg, "cron_min_interval_minutes", serde_json::json!(5))
            .await
            .as_i64()
            .unwrap_or(5),
    }))
    .into_response()
}

async fn audit_retention_days(pg: &sqlx::PgPool) -> i64 {
    // TS's DEFAULT_RETENTION_DAYS (audit.ts) — 90, not 0: an instance that
    // never set the knob keeps a quarter of history, not none.
    get_setting(pg, "audit_retention_days", serde_json::json!(90))
        .await
        .as_i64()
        .unwrap_or(90)
}

/// The PUT body's five independent writes, in TS's order (budgets first —
/// the governance control — then cron floor, retention, member models, org).
/// Every bound here REJECTS, like zod: a present-but-wrong field is a 400,
/// never a silent skip.
struct PutBody {
    audit_retention_days: Option<i64>,
    /// org: {} is truthy in TS — it runs (a no-op write, a roll, an audit
    /// with an empty after), so presence is tracked apart from the fields.
    org_present: bool,
    org_name: Option<String>,
    org_about: Option<String>,
    member_models: Option<Vec<String>>,
    llm_budgets: Option<Value>,
    /// The audit's after echoes the PARSED body — agents null entries still
    /// in — while `llm_budgets` is the null-filtered shape that gets stored.
    llm_budgets_after: Option<Value>,
    cron_min_interval_minutes: Option<i64>,
}

/// budgetLimits = z.object({ tokens: int(0..1e12).nullish(),
/// usd: number(0..1e9).nullish() }).nullable().optional() — every rejection
/// carries zod's own message, and the numbers pass through as the ORIGINAL
/// JSON (re-serializing from f64 would write 100000.0 where TS writes
/// 100000).
fn budget_limits(v: Option<&Value>) -> Result<Option<Value>, String> {
    let Some(v) = v else { return Ok(None) }; // .optional()
    if v.is_null() {
        return Ok(None); // .nullable()
    }
    let o = v
        .as_object()
        .ok_or_else(|| crate::body::object_msg(zod_type_name(v)))?;
    // tokens is z.number().int() — a fraction fails it; 24.0-style floats
    // carry an integer value and PASS.
    let field =
        |o: &serde_json::Map<String, Value>, key: &str, hi: f64, int: bool| -> Result<(), String>
        {
            match o.get(key) {
                None | Some(Value::Null) => Ok(()), // nullish — kept verbatim below
                Some(n) => {
                    let f = n.as_f64().ok_or_else(|| {
                        format!("Invalid input: expected number, received {}", zod_type_name(n))
                    })?;
                    if int && f.fract() != 0.0 {
                        return Err("Invalid input: expected int, received number".into());
                    }
                    if f < 0.0 {
                        return Err("Too small: expected number to be >=0".into());
                    }
                    if f > hi {
                        return Err(format!(
                            "Too big: expected number to be <={}",
                            crate::body::fmt_bound(hi)
                        ));
                    }
                    Ok(())
                }
            }
        };
    field(o, "tokens", 1e12, true)?;
    field(o, "usd", 1e9, false)?;
    let mut out = serde_json::Map::new();
    if let Some(t) = o.get("tokens") {
        out.insert("tokens".into(), t.clone());
    }
    if let Some(u) = o.get("usd") {
        out.insert("usd".into(), u.clone());
    }
    Ok(Some(Value::Object(out)))
}

fn parse_put_body(obj: &serde_json::Map<String, Value>) -> Result<PutBody, String> {
    use crate::body::{
        NumKind, array_msg, array_too_big_msg, number_member, object_msg, optional_number_member,
        string_msg, too_big_msg, too_small_msg, zod_type_name,
    };
    // Keys in the schema's order — zod surfaces the FIRST key's issue.
    let audit_retention_days = optional_number_member(
        obj,
        "auditRetentionDays",
        NumKind::Int,
        0.0,
        3650.0,
    )?
    .map(|f| f as i64);
    let (org_present, org_name, org_about) = match obj.get("org") {
        None => (false, None, None),
        Some(v) => {
            let o = v
                .as_object()
                .ok_or_else(|| object_msg(zod_type_name(v)))?;
            let name = match o.get("name") {
                None => None,
                Some(n) => {
                    let s = n.as_str().ok_or_else(|| string_msg(zod_type_name(n)))?;
                    if utf16_len(s) > 120 {
                        return Err(too_big_msg(120));
                    }
                    Some(s.to_string())
                }
            };
            let about = match o.get("about") {
                None => None,
                Some(a) => {
                    let s = a.as_str().ok_or_else(|| string_msg(zod_type_name(a)))?;
                    if utf16_len(s) > 2000 {
                        return Err(too_big_msg(2000));
                    }
                    Some(s.to_string())
                }
            };
            (true, name, about)
        }
    };
    let member_models = match obj.get("memberModels") {
        None => None,
        Some(v) => {
            let a = v
                .as_array()
                .ok_or_else(|| array_msg(zod_type_name(v)))?;
            if a.len() > 200 {
                return Err(array_too_big_msg(200));
            }
            let mut out = Vec::with_capacity(a.len());
            for m in a {
                let s = m.as_str().ok_or_else(|| string_msg(zod_type_name(m)))?;
                if utf16_len(s) < 1 {
                    return Err(too_small_msg(1));
                }
                if utf16_len(s) > 200 {
                    return Err(too_big_msg(200));
                }
                out.push(s.to_string());
            }
            Some(out)
        }
    };
    // llmBudgets, when present, is rewritten to the NORMALIZED shape zod
    // hands TS: all four keys, in order, agents null-filtered.
    let mut llm_after: Option<Value> = None;
    let llm_budgets = match obj.get("llmBudgets") {
        None => None,
        Some(b) => {
            let o = b
                .as_object()
                .ok_or_else(|| object_msg(zod_type_name(b)))?;
            // z.number().int(): 24 passes, 24.5 fails, 24.0 passes too —
            // and the ORIGINAL number passes through to storage.
            let window_hours =
                number_member(o, "windowHours", NumKind::Int, 1.0, 8760.0)?;
            // zod's .default(): absent org/perAgent → null, absent agents → {}.
            let org = budget_limits(o.get("org"))?;
            let per_agent = budget_limits(o.get("perAgent"))?;
            let mut after_agents = serde_json::Map::new();
            let mut stored_agents = serde_json::Map::new();
            match o.get("agents") {
                None | Some(Value::Null) => {}
                Some(v) => {
                    let a = v
                        .as_object()
                        .ok_or_else(|| crate::body::record_msg(zod_type_name(v)))?;
                    for (name, limits) in a {
                        if utf16_len(name) < 1 {
                            return Err(too_small_msg(1));
                        }
                        if utf16_len(name) > 200 {
                            return Err(too_big_msg(200));
                        }
                        let validated = budget_limits(Some(limits))?;
                        // The AUDIT after keeps null entries (it echoes the
                        // parsed body); the STORED value filters them.
                        after_agents.insert(name.clone(), validated.clone().unwrap_or(Value::Null));
                        if !limits.is_null() {
                            stored_agents.insert(name.clone(), validated.unwrap());
                        }
                    }
                }
            }
            let assembled = |agents: serde_json::Map<String, Value>| {
                Value::Object({
                    let mut m = serde_json::Map::new();
                    m.insert("windowHours".into(), crate::body::js_num(window_hours).into());
                    m.insert("org".into(), org.clone().unwrap_or(Value::Null));
                    m.insert("perAgent".into(), per_agent.clone().unwrap_or(Value::Null));
                    m.insert("agents".into(), Value::Object(agents));
                    m
                })
            };
            llm_after = Some(assembled(after_agents));
            Some(assembled(stored_agents))
        }
    };
    let cron_min_interval_minutes = optional_number_member(
        obj,
        "cronMinIntervalMinutes",
        NumKind::Int,
        0.0,
        1440.0,
    )?
    .map(|f| f as i64);
    Ok(PutBody {
        audit_retention_days,
        org_present,
        org_name,
        org_about,
        member_models,
        llm_budgets,
        llm_budgets_after: llm_after,
        cron_min_interval_minutes,
    })
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
    let body = match parse_put_body(obj) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Some(budgets) = &body.llm_budgets {
        // A spend ceiling is a governance control: who moved it, and from
        // what.
        let before = get_setting(
            &state.pg,
            "llm_budgets",
            serde_json::json!({"windowHours": 24, "org": null, "perAgent": null, "agents": {}}),
        )
        .await;
        if let Err(e) = set_setting(&state.pg, "llm_budgets", budgets).await {
            tracing::error!("[admin/settings] budgets write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "settings.llm_budgets",
                target_type: "settings",
                target_id: None,
                target_label: None,
                before: Some(before),
                after: body.llm_budgets_after.clone(),
            },
        )
        .await;
    }

    if let Some(minutes) = body.cron_min_interval_minutes {
        if let Err(e) = set_setting(
            &state.pg,
            "cron_min_interval_minutes",
            &serde_json::json!(minutes),
        )
        .await
        {
            tracing::error!("[admin/settings] cron floor write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "settings.cron_min_interval",
                target_type: "settings",
                target_id: None,
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "cronMinIntervalMinutes": minutes })),
            },
        )
        .await;
    }

    if let Some(days) = body.audit_retention_days {
        if let Err(e) = set_setting(&state.pg, "audit_retention_days", &serde_json::json!(days)).await {
            tracing::error!("[admin/settings] retention write failed: {e}");
            return thrown_internal_error();
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "settings.audit_retention",
                target_type: "settings",
                target_id: None,
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "auditRetentionDays": days })),
            },
        )
        .await;
    }

    if let Some(models) = &body.member_models {
        crate::model_access::set_member_model_allowlist(&state.pg, models).await;
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "settings.member_models",
                target_type: "settings",
                target_id: None,
                target_label: None,
                before: None,
                after: Some(serde_json::json!({ "memberModels": models })),
            },
        )
        .await;
    }

    if body.org_present {
        crate::org::set_org_profile(
            &state.pg,
            body.org_name.as_deref(),
            body.org_about.as_deref(),
        )
        .await;
        // The org lives in every rendered soul — propagate by ROLLING running
        // agents (new container up + healthy before the old one retires), so
        // an identity edit never kills anyone's in-flight conversation.
        // `void rollRunningAgents().catch(() => {})` — detached, failures
        // swallowed; the setting write already succeeded.
        let pg = state.pg.clone();
        let sb = state.secretbox().await.ok();
        tokio::spawn(async move {
            if let Some(sb) = sb {
                let _ = crate::fleet_reconcile::roll_running_agents(&pg, &sb).await;
            }
        });
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor,
                action: "settings.org",
                target_type: "settings",
                target_id: None,
                target_label: None,
                before: None,
                after: Some({
                    let mut m = serde_json::Map::new();
                    if let Some(n) = &body.org_name {
                        m.insert("name".into(), serde_json::json!(n));
                    }
                    if let Some(a) = &body.org_about {
                        m.insert("about".into(), serde_json::json!(a));
                    }
                    Value::Object(m)
                }),
            },
        )
        .await;
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}

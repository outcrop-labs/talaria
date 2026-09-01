// /api/admin/model-fitness — port of ui/src/routes/api/admin.model-fitness.ts.
// Admin → Models → Fitness, over HTTP. THIS FILE IS PLUMBING: the admin gate,
// the query string, the zod body, the audit line, the status code. Every
// decision — what a capability tag says across a pooled endpoint set, what a
// run will cost, what the archive keeps and evicts, what the drill-down shows
// — lives in `src/fitness/surface.rs`.
//
// The POST body is the zod union: start / stop / forget / clear. Each arm
// discriminates on a literal `action`, so the classify mirrors zod's smart
// union — the arm whose literal matches is the arm whose field errors the
// admin reads, and a body no arm claims is the union's own "Invalid input".

use std::collections::HashMap;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, js_numberify, parse};
use crate::error::house_error;
use crate::fitness::surface::{
    FitnessQuery, RunRefusal, StartOutcome, StartRequest, TierId, clear_fitness_results,
    forget_model, read_fitness, real_deps, start_fitness_run, stop_fitness_run,
};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

// GET  ?view=matrix (default) → slots + models + capability facts + cells + runs
//      ?view=capabilities     → models + facts only (the model pickers)
//      ?view=detail&model=    → one archived report + production telemetry
//      ?view=value            → price against performance, over the measured workload
//      ?view=estimate&model=&tiers=&adversary= → what a run will cost
//      ?view=transcripts&model=&run=    → every case of one run, in full (audit)
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let query = FitnessQuery {
        view: q.get("view").cloned().unwrap_or_else(|| "matrix".into()),
        model: q.get("model").cloned(),
        tiers: q.get("tiers").cloned(),
        adversary: q.get("adversary").cloned(),
        only: q.get("only").cloned(),
        reprobe: q.get("reprobe").map(|s| s == "1").unwrap_or(false),
        run: q.get("run").cloned(),
    };
    let deps = real_deps(&state);
    match read_fitness(&query, &state.pg, &deps).await {
        // The whole plane's wire is JS-printed — scores and ratios compute as
        // f64 here and `1.0` is not what TS sends for one (`js_numberify`).
        Ok(mut body) => {
            js_numberify(&mut body);
            Json(body).into_response()
        }
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

// ── the POST body: z.union's four arms ───────────────────────────────────────

#[derive(Debug, PartialEq)]
enum PostIntent {
    Start {
        model: String,
        tiers: Vec<TierId>,
        adversary_model: Option<String>,
        only: Option<Vec<String>>,
        restart: bool,
        reprobe: bool,
        concurrency: Option<usize>,
        retry_failed: bool,
        supplement: bool,
    },
    // A model stops ONE run; omitted stops every run in flight.
    Stop(Option<String>),
    Forget(String),
    // CLEAR IS NOT FORGET. Forget drops what a model CAN DO (probe facts,
    // paid for once and true until the id is re-pointed); this drops what a
    // RUN FOUND, so a candidate can be swept again from nothing. `model: null`
    // clears every tested candidate.
    Clear(Option<String>),
}

fn tier_of(s: &str) -> Option<TierId> {
    match s {
        "probes" => Some(TierId::Probes),
        "evals" => Some(TierId::Evals),
        "adversarial" => Some(TierId::Adversarial),
        _ => None,
    }
}

// THE ERROR CONTRACT, probed against the route's own `z.union` (zod v4):
// every arm discriminates on a literal `action`, so the union selects exactly
// one arm — and then the two failure kinds part ways. A PARSE failure (wrong
// JSON type anywhere, a missing required field, a tier element outside the
// enum, a non-int concurrency) kills the arm, the union tries its other arms,
// nothing claims the body, and the answer is the union's own `"Invalid input"`
// — even when another field ALSO failed a bound. A CHECK failure on the
// claimed arm (a length or range bound on an otherwise-parsed value) surfaces
// that arm's own message, first failing field in declaration order. So this
// classify parses everything first and only then applies the bounds.
fn classify_post(obj: &serde_json::Map<String, Value>) -> Result<PostIntent, String> {
    let action = match obj.get("action") {
        Some(Value::String(s)) => s.as_str(),
        _ => return Err("Invalid input".into()),
    };
    match action {
        // { action: 'start', model, tiers, adversaryModel?, only?, restart?,
        //   reprobe?, concurrency?, retryFailed?, supplement? }
        "start" => {
            // Pass one — parse. Any of these refusals is the union's own.
            let model = match obj.get("model") {
                Some(Value::String(s)) => s.as_str(),
                _ => return Err("Invalid input".into()),
            };
            let tiers_arr = match obj.get("tiers") {
                Some(Value::Array(a)) => a,
                _ => return Err("Invalid input".into()),
            };
            let mut tiers = Vec::with_capacity(tiers_arr.len());
            for el in tiers_arr {
                // An unknown tier is an ENUM failure, not a bound — it parses
                // nothing, so the union answers, not the arm.
                let Some(t) = el.as_str().and_then(tier_of) else {
                    return Err("Invalid input".into());
                };
                tiers.push(t);
            }
            let adversary_model = match obj.get("adversaryModel") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.as_str()),
                _ => return Err("Invalid input".into()),
            };
            // `only` is `.optional()`, NOT nullish — an explicit null parses
            // as no array at all, which is a type error here (the adversary's
            // nullish is the contrast on purpose: the picker distinguishes
            // "no adversary" from an empty list).
            let only = match obj.get("only") {
                None => None,
                Some(Value::Array(a)) => Some(
                    a.iter()
                        .map(|el| el.as_str().ok_or("Invalid input").map(str::to_string))
                        .collect::<Result<Vec<String>, _>>()?,
                ),
                _ => return Err("Invalid input".into()),
            };
            let flag = |key: &str| -> Result<bool, String> {
                match obj.get(key) {
                    None => Ok(false),
                    Some(Value::Bool(b)) => Ok(*b),
                    _ => Err("Invalid input".into()),
                }
            };
            let concurrency = match obj.get("concurrency") {
                None => None,
                // A whole float is a valid int to zod (1.0 === 1); anything
                // with a fraction is not an int and never reaches the bounds.
                Some(Value::Number(n)) => match n.as_f64() {
                    Some(f) if f.fract() == 0.0 => Some(f),
                    _ => return Err("Invalid input".into()),
                },
                _ => return Err("Invalid input".into()),
            };
            // Pass two — bounds, first failing field in the arm's order:
            // model, tiers, adversaryModel, only, concurrency.
            let utf16 = crate::body::utf16_len;
            if utf16(model) < 1 {
                return Err("Too small: expected string to have >=1 characters".into());
            }
            if utf16(model) > 200 {
                return Err("Too big: expected string to have <=200 characters".into());
            }
            if tiers.is_empty() {
                return Err("Too small: expected array to have >=1 items".into());
            }
            if let Some(a) = adversary_model {
                if utf16(a) > 200 {
                    return Err("Too big: expected string to have <=200 characters".into());
                }
            }
            if let Some(items) = &only {
                for el in items {
                    if utf16(el) > 120 {
                        return Err("Too big: expected string to have <=120 characters".into());
                    }
                }
                if items.len() > 64 {
                    return Err("Too big: expected array to have <=64 items".into());
                }
            }
            if let Some(c) = concurrency {
                if c < 1.0 {
                    return Err("Too small: expected number to be >=1".into());
                }
                if c > 8.0 {
                    return Err("Too big: expected number to be <=8".into());
                }
            }
            Ok(PostIntent::Start {
                model: model.to_string(),
                tiers,
                adversary_model: adversary_model.map(str::to_string),
                only,
                restart: flag("restart")?,
                reprobe: flag("reprobe")?,
                concurrency: concurrency.map(|c| c as usize),
                retry_failed: flag("retryFailed")?,
                supplement: flag("supplement")?,
            })
        }
        // { action: 'stop', model?: string | null } — max only, no min: the
        // empty string stops every run, same as null (a blanked picker is
        // "all", not a malformed request).
        "stop" => Ok(PostIntent::Stop(nullish_max("model", obj)?)),
        // { action: 'forget', model } — required, min 1: forgetting "every
        // model" is clear-all's job.
        "forget" => {
            let model = match obj.get("model") {
                Some(Value::String(s)) => s,
                _ => return Err("Invalid input".into()),
            };
            if crate::body::utf16_len(model) < 1 {
                return Err("Too small: expected string to have >=1 characters".into());
            }
            if crate::body::utf16_len(model) > 200 {
                return Err("Too big: expected string to have <=200 characters".into());
            }
            Ok(PostIntent::Forget(model.to_string()))
        }
        // { action: 'clear', model?: string | null } — stop's shape.
        "clear" => Ok(PostIntent::Clear(nullish_max("model", obj)?)),
        _ => Err("Invalid input".into()),
    }
}

/// The stop/clear `model` member — `z.string().max(200).nullish()`: null,
/// absent, and the empty string all mean "every model", and only a string
/// longer than 200 is refused.
fn nullish_max(key: &str, obj: &serde_json::Map<String, Value>) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if crate::body::utf16_len(s) > 200 {
                Err("Too big: expected string to have <=200 characters".into())
            } else {
                Ok(Some(s.to_string()))
            }
        }
        _ => Err("Invalid input".into()),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
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
    let actor = actor_of(&user);
    match classify_post(obj) {
        // `{ stopped, status, runs }` — two things can be running per candidate
        // (the tier-2 sweep and the tier loop) and `stop_fitness_run` signals
        // both.
        Ok(PostIntent::Stop(model)) => {
            let deps = real_deps(&state);
            let result = stop_fitness_run(model.as_deref(), &deps).await;
            audit(
                &state,
                &actor,
                "model_fitness.stop",
                "model-fitness",
                model.as_deref().unwrap_or("all"),
                None,
            );
            wire(json!({
                "stopped": result.stopped,
                "status": result.status,
                "runs": result.runs,
            }))
        }
        Ok(PostIntent::Clear(model)) => {
            let deps = real_deps(&state);
            let cleared = match clear_fitness_results(model.as_deref(), &deps).await {
                Ok(c) => c,
                Err(e) => return house_error(StatusCode::BAD_REQUEST, &e),
            };
            audit(
                &state,
                &actor,
                "model_fitness.clear",
                "model-fitness",
                model.as_deref().unwrap_or("all"),
                Some(json!({
                    "models": cleared.models,
                    "reports": cleared.reports,
                    "transcripts": cleared.transcripts,
                })),
            );
            wire(json!({
                "models": cleared.models,
                "reports": cleared.reports,
                "transcripts": cleared.transcripts,
            }))
        }
        Ok(PostIntent::Forget(model)) => {
            let deps = real_deps(&state);
            let result = match forget_model(&model, &deps).await {
                Ok(r) => r,
                Err(e) => return house_error(StatusCode::BAD_REQUEST, &e),
            };
            audit(
                &state,
                &actor,
                "model_fitness.forget",
                "model",
                &model,
                Some(json!({ "keys": result.keys, "report": result.report })),
            );
            wire(json!({ "models": result.models, "report": result.report }))
        }
        Ok(PostIntent::Start {
            model,
            tiers,
            adversary_model,
            only,
            restart,
            reprobe,
            concurrency,
            retry_failed,
            supplement,
        }) => {
            let deps = std::sync::Arc::new(real_deps(&state));
            let started = start_fitness_run(
                StartRequest {
                    model: model.clone(),
                    tiers: tiers.clone(),
                    adversary_model: adversary_model.clone(),
                    only,
                    restart,
                    reprobe,
                    concurrency,
                    retry_failed,
                    supplement,
                },
                deps,
            )
            .await;
            match started {
                // 409 means "a run is already in flight, here it is" — the
                // second press of Start shows the run rather than buying a
                // second one. 400 is a request that could never have run.
                StartOutcome::Busy { refusal, status, runs } => {
                    let error = if refusal == RunRefusal::AtCapacity {
                        format!(
                            "already testing {} models — stop one, or wait for it to finish",
                            runs.max
                        )
                    } else {
                        "that model is already being tested".to_string()
                    };
                    let mut body = json!({
                        "started": false,
                        "refusal": refusal.as_str(),
                        "error": error,
                        "status": status,
                        "runs": runs,
                    });
                    js_numberify(&mut body);
                    (StatusCode::CONFLICT, Json(body)).into_response()
                }
                StartOutcome::Rejected(e) => house_error(StatusCode::BAD_REQUEST, &e),
                StartOutcome::Started { status, runs } => {
                    audit(
                        &state,
                        &actor,
                        "model_fitness.start",
                        "model",
                        &model,
                        Some(json!({
                            "tiers": tiers,
                            "adversary": adversary_model,
                            "reprobe": reprobe,
                        })),
                    );
                    wire(json!({ "started": true, "status": status, "runs": runs }))
                }
            }
        }
        Err(msg) => house_error(StatusCode::BAD_REQUEST, &msg),
    }
}

/// The POST response, JS-printed for the same reason as the GET's (the
/// status row and run rows ride through as stored values, and a whole float
/// stored as `1.0` reads `1` from TS).
fn wire(mut body: Value) -> Response {
    js_numberify(&mut body);
    Json(body).into_response()
}

/// `void logAudit(...)` — the audit line never blocks or breaks the verb.
fn audit(
    state: &AppState,
    actor: &str,
    action: &'static str,
    target_type: &'static str,
    target_id: &str,
    after: Option<Value>,
) {
    let pg = state.pg.clone();
    let actor = actor.to_string();
    let target_id = target_id.to_string();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action,
                target_type,
                target_id: Some(&target_id),
                target_label: None,
                before: None,
                after,
            },
        )
        .await;
    });
}

// The one decision this file makes is the union dispatch, so the one thing it
// tests is that dispatch — the same probe-answer pinning style as
// admin_rag's table.
#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        match v {
            Value::Object(m) => m,
            _ => unreachable!("the fixture is an object"),
        }
    }

    #[test]
    fn the_post_union_dispatches_on_the_action_literal() {
        assert!(matches!(
            classify_post(&obj(json!({"action": "stop"}))),
            Ok(PostIntent::Stop(None))
        ));
        assert!(matches!(
            classify_post(&obj(json!({"action": "stop", "model": "m1"}))),
            Ok(PostIntent::Stop(Some(m))) if m == "m1"
        ));
        assert!(matches!(
            classify_post(&obj(json!({"action": "stop", "model": null}))),
            Ok(PostIntent::Stop(None))
        ));
        assert!(matches!(
            classify_post(&obj(json!({"action": "clear", "model": null}))),
            Ok(PostIntent::Clear(None))
        ));
        assert!(matches!(
            classify_post(&obj(json!({"action": "forget", "model": "m1"}))),
            Ok(PostIntent::Forget(m)) if m == "m1"
        ));
        // No arm claims these: the union's own error.
        assert_eq!(
            classify_post(&obj(json!({"action": "nope"}))),
            Err("Invalid input".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"model": "m1"}))),
            Err("Invalid input".into())
        );
        assert_eq!(classify_post(&obj(json!({}))), Err("Invalid input".into()));
    }

    #[test]
    fn the_start_arm_reads_every_knob_and_defaults_the_unticked_ones() {
        let Ok(PostIntent::Start {
            model,
            tiers,
            adversary_model,
            only,
            restart,
            reprobe,
            concurrency,
            retry_failed,
            supplement,
        }) = classify_post(&obj(json!({
            "action": "start",
            "model": "qwen3-14b",
            "tiers": ["probes", "adversarial"],
            "adversaryModel": null,
        })))
        else {
            panic!("the start arm claims a bare start");
        };
        assert_eq!(model, "qwen3-14b");
        assert_eq!(tiers.len(), 2);
        assert_eq!(adversary_model, None);
        assert_eq!(only, None);
        assert!(!restart);
        assert!(!reprobe);
        assert_eq!(concurrency, None);
        assert!(!retry_failed);
        assert!(!supplement);

        let full = classify_post(&obj(json!({
            "action": "start",
            "model": "m",
            "tiers": ["evals"],
            "adversaryModel": "judge-1",
            "only": ["research.recon"],
            "restart": true,
            "reprobe": true,
            "concurrency": 4,
            "retryFailed": true,
            "supplement": true,
        })))
        .expect("every knob parses");
        let PostIntent::Start {
            adversary_model,
            only,
            concurrency,
            ..
        } = full
        else {
            unreachable!("classified above");
        };
        assert_eq!(adversary_model.as_deref(), Some("judge-1"));
        assert_eq!(only.as_deref(), Some(&["research.recon".to_string()][..]));
        assert_eq!(concurrency, Some(4));
    }

    #[test]
    fn the_start_arm_refuses_what_zod_refuses() {
        // The model bounds — checks, so the arm's own sentences.
        assert_eq!(
            classify_post(&obj(json!({"action": "start", "model": "", "tiers": ["probes"]}))),
            Err("Too small: expected string to have >=1 characters".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"action": "start", "model": "x".repeat(201), "tiers": ["probes"]}))),
            Err("Too big: expected string to have <=200 characters".into())
        );
        // The tiers array: min 1 is a check; an unknown tier is an enum
        // PARSE failure, which drags the whole body to the union's sentence —
        // even when the model is also over-long (probed: parse poisons bounds).
        assert_eq!(
            classify_post(&obj(json!({"action": "start", "model": "m", "tiers": []}))),
            Err("Too small: expected array to have >=1 items".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"action": "start", "model": "m", "tiers": ["nope"]}))),
            Err("Invalid input".into())
        );
        assert_eq!(
            classify_post(&obj(json!({"action": "start", "model": "", "tiers": ["nope"]}))),
            Err("Invalid input".into())
        );
        // concurrency: int bounds are checks, non-int is a parse failure.
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"], "concurrency": 9
            }))),
            Err("Too big: expected number to be <=8".into())
        );
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"], "concurrency": 0
            }))),
            Err("Too small: expected number to be >=1".into())
        );
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"], "concurrency": 2.5
            }))),
            Err("Invalid input".into())
        );
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"], "concurrency": true
            }))),
            Err("Invalid input".into())
        );
        // `only`: max-only elements (the empty string is legal), array cap 64.
        assert!(classify_post(&obj(json!({
            "action": "start", "model": "m", "tiers": ["probes"], "only": [""]
        })))
        .is_ok());
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"], "only": ["x".repeat(121)]
            }))),
            Err("Too big: expected string to have <=120 characters".into())
        );
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"],
                "only": (0..65).map(|_| "x").collect::<Vec<_>>()
            }))),
            Err("Too big: expected array to have <=64 items".into())
        );
        // only is optional, not nullish — a null is a type error, the union's.
        assert_eq!(
            classify_post(&obj(json!({"action": "start", "model": "m", "tiers": ["probes"], "only": null}))),
            Err("Invalid input".into())
        );
        // adversaryModel is nullish with a max and NO min: empty means "none".
        assert!(classify_post(&obj(json!({
            "action": "start", "model": "m", "tiers": ["probes"], "adversaryModel": ""
        })))
        .is_ok());
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "m", "tiers": ["probes"], "adversaryModel": "x".repeat(201)
            }))),
            Err("Too big: expected string to have <=200 characters".into())
        );
        // Bounds report in the arm's field order: model before adversaryModel
        // before only (probed against the route's own union).
        assert_eq!(
            classify_post(&obj(json!({
                "action": "start", "model": "x".repeat(201), "tiers": ["probes"],
                "adversaryModel": "x".repeat(201), "only": ["x".repeat(121)]
            }))),
            Err("Too big: expected string to have <=200 characters".into())
        );
    }
}

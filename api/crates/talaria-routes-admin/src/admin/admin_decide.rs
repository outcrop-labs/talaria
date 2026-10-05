// /api/admin/decide. The decision-model port's one config row (admin).
// GET → the provider catalog with its capability sheet, plus the current
// config, redacted. PUT → patch the config. POST { action: "test" } → put one
// real question through whatever is configured and report exactly what came
// back.
//
// WHY THE TEST ARM EARNS ITS KEEP. Every other field on this panel can be
// wrong in a way nothing notices: a URL that resolves but speaks a different
// protocol, a key with the wrong scope, a classifier whose label vocabulary we
// do not recognize, a chat endpoint that silently drops `logprobs`. All four
// present as "the port is on" and then as call sites quietly falling back
// forever. One round trip with a question whose answer a person can check
// turns every one of those into a sentence on the panel.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::decide::{
    self, ALL_WIRES, DECIDE_PROVIDERS, DecidePatch, Question, configured, providers_public, shadow,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    NumKind, optional_enum_member, optional_number_member, parse,
    present_nullable_max_string_member,
};
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let cfg = decide::get_decide_config(&state.pg).await;
    Ok(Json(json!({
        "providers": providers_public(),
        "wires": ALL_WIRES,
        "config": decide::decide_config_public(&state.pg).await,
        // Whether the panel should offer the Test button at all — computed
        // from the registry, so it cannot drift from what `decide` itself
        // will refuse.
        "configured": configured(&cfg),
        // THE MEASUREMENT, per wired site. Empty until a site has run
        // comparisons, which is every install by default — a provider being
        // configured is not the same as a site being switched over, and this
        // block is the evidence a person reads before switching one.
        "shadow": shadow::shadow_value(shadow::shadow_report(&state.pg).await),
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;

    // The enum list is the catalog's own order — 'off' is already its first
    // entry, so there is no second spelling of the off switch to keep in step.
    let ids: Vec<&str> = DECIDE_PROVIDERS.iter().map(|p| p.id).collect();
    let provider = match optional_enum_member(obj, "provider", &ids) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let url = match present_nullable_max_string_member(obj, "url", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let model = match present_nullable_max_string_member(obj, "model", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // A wire is a protocol we speak or it is a 400 — a config naming one we do
    // not would read as "configured" and then fail on every call.
    let wire = match obj.get("wire") {
        Some(serde_json::Value::Null) => Some(None),
        Some(_) => match optional_enum_member(obj, "wire", &ALL_WIRES) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
        None => None,
    };
    let api_key = match present_nullable_max_string_member(obj, "apiKey", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let timeout_ms = match optional_number_member(obj, "timeoutMs", NumKind::Int, 250.0, 60_000.0) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    let next = match decide::set_decide_config(
        &state,
        DecidePatch {
            provider,
            url,
            model,
            wire,
            api_key,
            timeout_ms: timeout_ms.map(|t| Some(t as i64)),
        },
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return Ok(talaria_error::internal(
                "[admin] set_decide_config failed",
                e,
            ));
        }
    };

    // The audit carries the shape, never the secret — `next` is the stored row
    // and the stored row holds a sealed key, so the entry is built from the
    // fields a reader needs rather than from the row.
    let after = json!({
        "provider": next.get("provider").and_then(serde_json::Value::as_str),
        "model": next.get("model").and_then(serde_json::Value::as_str),
        "wire": next.get("wire").and_then(serde_json::Value::as_str),
        "hasKey": next.get("keySealed").is_some(),
    });
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "settings.decide",
            target_type: "settings",
            target_id: Some("decide"),
            target_label: None,
            before: None,
            after: Some(after),
        },
    )
    .await;

    let cfg = decide::get_decide_config(&state.pg).await;
    Ok(Json(json!({
        "config": decide::decide_config_public(&state.pg).await,
        "configured": configured(&cfg),
    }))
    .into_response())
}

/// The probe question. Deliberately one a PERSON can check at a glance: the
/// state says the deploy is broken and nobody can ship, and the question asks
/// whether that is urgent. A provider answering near 1 is wired up correctly;
/// one answering near 0 is answering a different question than we think we are
/// asking, which is exactly the misconfiguration a 200 would otherwise hide.
fn probe() -> (serde_json::Value, Question) {
    (
        json!({
            "report": "The production deploy is failing and nobody on the team can ship.",
        }),
        Question::noul_meaning(
            "Does the report below describe something urgent?",
            "work is blocked or something is broken right now",
            "it can wait until later",
        ),
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    match optional_enum_member(obj, "action", &["test"]) {
        Ok(_) => {}
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    }

    let cfg = decide::get_decide_config(&state.pg).await;
    if !configured(&cfg) {
        // Not a 400: the body was fine, the instance is not configured, and
        // the panel wants to say so in its own words rather than render an
        // error envelope.
        return Ok(Json(json!({
            "ok": false,
            "reason": "No decision model is configured yet.",
        }))
        .into_response());
    }

    let (subject, question) = probe();
    let http = talaria_api_facades::retrieval::real_http();
    let answered = decide::ask_one(&state, &http, subject, question).await;

    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "settings.decide.test",
            target_type: "settings",
            target_id: Some("decide"),
            target_label: None,
            before: None,
            after: Some(json!({ "answered": answered.is_some() })),
        },
    )
    .await;

    Ok(Json(match answered {
        Some(j) => json!({
            "ok": true,
            "provider": j.provider,
            "model": j.model,
            // The probability for the probe's own question — near 1 is the
            // right answer, and the panel says so beside it.
            "probability": j.answer.probability(),
            "certainty": j.certainty(),
            // THE FIELD AN OPERATOR MOST NEEDS TO SEE. A `false` here means
            // the port works and every confidence-gated call site will still
            // fall back, because there is no real number behind the answer.
            "calibrated": j.calibrated,
            "latencyMs": j.latency_ms,
        }),
        None => json!({
            "ok": false,
            "reason": "The provider did not answer, or answered in a shape this wire does not recognize. Check the URL, the key, and that the model serves yes/no judgments.",
        }),
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_question_is_a_noul_whose_right_answer_a_person_can_check() {
        let (subject, question) = probe();
        assert_eq!(question.primitive(), "noul");
        // Both halves must carry meaning: the chat wire builds its YES/NO menu
        // out of `yes_no`, so a probe without one would be testing a different
        // prompt than production sends.
        let Question::Noul { yes_no, .. } = question else {
            panic!("the probe is a noul");
        };
        let (yes, no) = yes_no.expect("the probe spells out yes and no");
        assert!(yes.contains("blocked"), "{yes}");
        assert!(!no.is_empty());
        assert!(
            subject["report"]
                .as_str()
                .expect("a report")
                .contains("failing"),
            "the state has to actually describe the thing being judged"
        );
    }

    #[test]
    fn every_provider_id_is_offered_to_the_put_so_the_panel_cannot_send_an_unsettable_one() {
        let ids: Vec<&str> = DECIDE_PROVIDERS.iter().map(|p| p.id).collect();
        assert!(ids.contains(&"off"), "the off switch is settable");
        assert_eq!(ids[0], "off", "and it is the first option");
        for p in providers_public() {
            assert!(ids.contains(&p.id), "{} is shown but not settable", p.id);
        }
    }
}

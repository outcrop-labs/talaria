// /api/admin/decide. The decision-model port's one config row (admin).
// GET → the provider catalog with its capability sheet, the current config
// redacted, and the SITE CENSUS: every place in Talaria that asks a decision
// model anything. PUT → patch the config, or, with `{ site: { id, on?, floor? } }`,
// switch one site over and set the certainty it acts at — handled on its own
// and answering `{sites}`, because "which model" and "which of our judgments
// run on it" are different writes and should not share an audit entry. A site
// whose switch lives on another surface (the guardrail rule toggles, the
// rerank provider picker) is refused there with the sentence naming where it
// is. POST { action: "test" } → put one real question through whatever is
// configured and report exactly what came back. POST { action: "models" } →
// ask the endpoint which model ids it will accept.
//
// WHY THE TEST ARM EARNS ITS KEEP. Every other field on this panel can be
// wrong in a way nothing notices: a URL that resolves but speaks a different
// protocol, a key with the wrong scope, a classifier whose label vocabulary we
// do not recognize, a chat endpoint that silently drops `logprobs`. All four
// present as "the port is on" and then as call sites quietly falling back
// forever. One round trip with a question whose answer a person can check
// turns every one of those into a sentence on the panel.
//
// AND THE SENTENCE IS THE PROVIDER'S OWN. The first real configuration of this
// panel failed on a `model` of `jev` instead of `jev-latest`: a free-text field
// whose shortest plausible value is wrong, a Test button that answered with our
// generic "did not answer, or answered in a shape we do not recognize", and a
// `400 Unknown model: jev` sitting unread in the response body. Both halves of
// that are fixed here — the refusal is rendered verbatim, and `action: "models"`
// asks the endpoint for the ids it actually accepts so the field can be a list
// rather than a guess.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::decide::{
    self, ALL_WIRES, DECIDE_PROVIDERS, DecidePatch, NoAnswer, Question, configured,
    providers_public, shadow, sites, tools,
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
        // THE CENSUS. Every place in Talaria that asks a decision model
        // anything, what it does with the answer, where it is switched on, and
        // the floor it acts at - so "where is this being used" has one
        // complete answer rather than a grep.
        "sites": sites::sites_public(&state.pg).await,
        // Tool-offer pruning's measurement, which has its OWN switch:
        // configuring a decision model is not consent to put every agent turn
        // through a question per offered tool. See talaria-decide::tools.
        "toolShadow": tools::shadow_enabled(&state.pg).await,
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
    // -- THE SITE SWITCH ----------------------------------------------------
    //
    // Handled before the config patch and RETURNED from, because the two are
    // different writes: `decide_config` says which model, `decide_sites` says
    // which of Talaria's own judgments run on it and at what certainty. A
    // single body setting both would make "switch this site on" and "change
    // provider" the same audit entry.
    if let Some(site) = obj.get("site") {
        let Some(site) = site.as_object() else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "site must be an object",
            ));
        };
        let id = match talaria_body::string_member(site, "id", 1, 80) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        let on = match site.get("on") {
            Some(_) => match talaria_body::boolean_member(site, "on") {
                Ok(v) => Some(v),
                Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
            },
            None => None,
        };
        let floor = match optional_number_member(site, "floor", NumKind::Float, 0.0, 1.0) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
        // A site whose switch lives on another surface refuses HERE with the
        // sentence naming where it is - a second spelling of one switch is how
        // the two come to disagree.
        if let Err(msg) = sites::set_site(&state.pg, &id, on, floor).await {
            return Ok(house_error(StatusCode::BAD_REQUEST, &msg));
        }
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "settings.decide.site",
                target_type: "settings",
                target_id: Some("decide"),
                target_label: Some(&id),
                before: None,
                after: Some(json!({ "on": on, "floor": floor })),
            },
        )
        .await;
        return Ok(Json(json!({ "sites": sites::sites_public(&state.pg).await })).into_response());
    }

    // Its own key rather than a field on the config row: the measurement is
    // not part of "which decision model", and an operator switching providers
    // should not silently re-enable it.
    let tool_shadow = match obj.get("toolShadow") {
        Some(_) => match talaria_body::boolean_member(obj, "toolShadow") {
            Ok(v) => Some(v),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
        None => None,
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

    if let Some(on) = tool_shadow {
        talaria_api_facades::gateway::settings::set_setting(&state.pg, tools::SETTING, &json!(on))
            .await
            .ok();
    }

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
        "toolShadow": tools::shadow_enabled(&state.pg).await,
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
    let action = match optional_enum_member(obj, "action", &["test", "models"]) {
        Ok(v) => v.unwrap_or_else(|| "test".to_string()),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    let cfg = decide::get_decide_config(&state.pg).await;
    if !configured(&cfg) {
        // Not a 400: the body was fine, the instance is not configured, and
        // the panel wants to say so in its own words rather than render an
        // error envelope.
        return Ok(Json(json!({
            "ok": false,
            "kind": NoAnswer::NotConfigured.kind(),
            "reason": NoAnswer::NotConfigured.sentence(),
        }))
        .into_response());
    }

    let http = talaria_api_facades::retrieval::real_http();

    // The catalog read is not audited: it names no model, changes nothing, and
    // an admin pressing it twice is how they check whether a key works.
    if action == "models" {
        return Ok(Json(match decide::list_models(&state, &http).await {
            Ok(models) => json!({ "ok": true, "models": models }),
            Err(e) => json!({ "ok": false, "kind": e.kind(), "reason": e.sentence() }),
        })
        .into_response());
    }

    let (subject, question) = probe();
    let ask = decide::Ask::new(subject).q("q", question);
    // A 2xx that carried no answer for the one id we asked under is the
    // `Unreadable` case — the same thing a wire not recognizing the body is.
    let answered = decide::try_decide(&state, &http, &ask)
        .await
        .and_then(|mut m| m.remove("q").ok_or(NoAnswer::Unreadable));

    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "settings.decide.test",
            target_type: "settings",
            target_id: Some("decide"),
            target_label: None,
            before: None,
            after: Some(json!({ "answered": answered.is_ok() })),
        },
    )
    .await;

    Ok(Json(match answered {
        Ok(j) => json!({
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
        // THE PROVIDER'S OWN WORDS, not ours. `kind` is the stable tag so the
        // panel can style a refusal differently from an unreachable host
        // without matching on prose.
        Err(e) => json!({
            "ok": false,
            "kind": e.kind(),
            "reason": e.sentence(),
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
    fn a_refusal_carries_the_providers_own_words_and_a_tag_the_panel_can_branch_on() {
        // The sentence a person reads has to be the ENDPOINT'S. This is the
        // literal case that produced the fix: `model` was `jev`, TypeSafe
        // answered 400 with `Unknown model: jev`, and the panel said nothing
        // about it.
        let refused = NoAnswer::Refused {
            status: 400,
            message: "Unknown model: jev".into(),
        };
        assert_eq!(refused.kind(), "refused");
        let sentence = refused.sentence();
        assert!(sentence.contains("Unknown model: jev"), "{sentence}");
        assert!(
            sentence.contains("400"),
            "the status is context: {sentence}"
        );

        // And every kind is a distinct tag — the panel styles an unreachable
        // host differently from a refusal, and matching on prose would break
        // the moment a sentence is reworded.
        let kinds = [
            NoAnswer::NotConfigured.kind(),
            NoAnswer::Unusable("x".into()).kind(),
            NoAnswer::Unreachable("x".into()).kind(),
            refused.kind(),
            NoAnswer::Unreadable.kind(),
        ];
        let mut sorted = kinds.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), kinds.len(), "two kinds share a tag");
        for k in kinds {
            assert!(!k.is_empty());
        }
        // The not-configured sentence is the one the route sends before it
        // ever dispatches, so it must stand on its own.
        assert!(
            NoAnswer::NotConfigured
                .sentence()
                .contains("No decision model"),
        );
    }

    #[test]
    fn the_census_covers_every_site_the_panel_can_switch_and_names_where_the_others_live() {
        // The panel renders a checkbox for a site it can switch and a sentence
        // for one it cannot. A site that is neither would render a dead
        // control, so every entry must answer the question one way.
        let mut switchable = 0;
        for s in sites::DECIDE_SITES {
            match s.switch_lives_at {
                None => switchable += 1,
                Some(w) => assert!(
                    !w.trim().is_empty(),
                    "{} says its switch is elsewhere without saying where",
                    s.id
                ),
            }
        }
        assert!(
            switchable >= 3,
            "the panel owns at least the three wired sites"
        );
        // And `set_site` refuses the ones it does not own rather than writing a
        // second switch beside the real one.
        for s in sites::DECIDE_SITES
            .iter()
            .filter(|s| s.switch_lives_at.is_some())
        {
            assert!(
                sites::def_of(s.id).is_some_and(|d| d.switch_lives_at.is_some()),
                "{} must stay refused by set_site",
                s.id
            );
        }
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

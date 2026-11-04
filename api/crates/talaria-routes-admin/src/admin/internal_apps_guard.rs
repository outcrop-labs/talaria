// doc: The app host hands over the final reply of one harness run an app's
// doc: code made, for the guard's semantic rules — the ones a pattern cannot
// doc: answer and the operator's decision model judges (`implied_completion`:
// doc: does the reply claim something was done that nothing in its tool record
// doc: did?). `{ answer, tools, model, caller?, rules }`: `tools` is the run's
// doc: tool record (the backing tools the host's own structural pass read),
// doc: `rules` the semantic rules the harness's `guard.rules` keeps (all of
// doc: them when it declares none) and an admin has on — the host asks
// doc: nothing when that leaves none. The host runs every structural rule
// doc: itself and the relay runs none for an app's key, so each rule is judged
// doc: once: the structural ones there, these here. A rule is judged only
// doc: while an admin has it on and a decision model is configured; the pass
// doc: runs detached, after the reply has gone back to the app, so it records
// doc: findings under the run's caller stamp (kept inside `app:<slug>`) and
// doc: can never change the answer. Answers 202 with the rules it took; 404
// doc: when the app is not enabled. Callable only with the root-derived
// doc: service credential (`x-talaria-internal`); the host's proxy never
// doc: forwards `/api/internal/*`.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::decide::guard::{observe_among, semantic_rule_ids};
use talaria_api_facades::gateway::app_keys::app_caller_stamp;
use talaria_apps::enabled_app_slugs;
use talaria_error::house_error;
use talaria_session::require_internal;
use talaria_state::AppState;

/// The longest reply judged, in characters. A decision model reads the whole
/// state; a reply past this is judged on its head, which is where a claim of
/// finished work sits.
const ANSWER_MAX: usize = 32_000;
/// A tool record past this many names is not a turn's record.
const TOOLS_MAX: usize = 256;
const NAME_MAX: usize = 200;

/// One run's reply, as the host sent it, held to bounds.
#[derive(Debug, PartialEq)]
struct Judging {
    answer: String,
    tools: Vec<String>,
    model: String,
    caller: Option<String>,
    rules: Vec<String>,
}

/// The body, or the sentence a 400 carries. `rules` is narrowed to the rules
/// this pass answers: a structural id is the host's to run, never this
/// door's, and an unknown one is nothing to run.
fn judging_of(body: &Value) -> Result<Judging, String> {
    let answer = body["answer"]
        .as_str()
        .ok_or("answer must be the run's reply text")?;
    let model = body["model"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty() && m.len() <= NAME_MAX)
        .ok_or("model must name the model the run was answered by")?;
    let names = |key: &str| -> Result<Vec<String>, String> {
        match &body[key] {
            Value::Null => Ok(Vec::new()),
            Value::Array(items) if items.len() <= TOOLS_MAX => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .filter(|s| !s.is_empty() && s.len() <= NAME_MAX)
                        .map(String::from)
                        .ok_or_else(|| format!("{key} must be a list of names"))
                })
                .collect(),
            _ => Err(format!("{key} must be a list of at most {TOOLS_MAX} names")),
        }
    };
    let tools = names("tools")?;
    let known = semantic_rule_ids();
    let mut rules: Vec<String> = Vec::new();
    for id in names("rules")? {
        if known.contains(&id.as_str()) && !rules.contains(&id) {
            rules.push(id);
        }
    }
    Ok(Judging {
        answer: answer.chars().take(ANSWER_MAX).collect(),
        tools,
        model: model.to_string(),
        caller: body["caller"].as_str().map(String::from),
        rules,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    require_internal(&state, &headers)?;
    if !enabled_app_slugs(&state.pg).await.contains(&slug) {
        return Ok(house_error(
            StatusCode::NOT_FOUND,
            &format!("app \"{slug}\" is not enabled, so it runs no harness to guard"),
        ));
    }
    let judging = match judging_of(&talaria_body::parse(&body)) {
        Ok(j) => j,
        Err(why) => return Ok(house_error(StatusCode::BAD_REQUEST, &why)),
    };
    if !judging.rules.is_empty() && !judging.answer.trim().is_empty() {
        // The same stamp the relay books the run's spend under, so a finding
        // and the run's ledger rows name one caller.
        let caller = app_caller_stamp(&slug, judging.caller.as_deref());
        observe_among(
            &state,
            &judging.answer,
            &judging.tools,
            &caller,
            &judging.model,
            Some(judging.rules.clone()),
        );
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "ok": true, "rules": judging.rules })),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_taken_are_the_semantic_ones_the_harness_kept() {
        let j = judging_of(&json!({
            "answer": "Deployed.",
            "model": "mock-1",
            "rules": ["implied_completion", "zero_tool_claim", "no_such_rule", "implied_completion"],
        }))
        .expect("a well-formed body");
        // A structural id is the host's to run; an unknown one is nothing;
        // a repeat is judged once.
        assert_eq!(j.rules, vec!["implied_completion".to_string()]);
        assert!(j.tools.is_empty(), "no tool record reads as an empty one");
    }

    #[test]
    fn a_harness_that_keeps_no_semantic_rule_asks_for_nothing() {
        let j = judging_of(&json!({
            "answer": "x", "model": "m", "rules": ["secret_leak"], "tools": ["read_file"],
        }))
        .expect("well-formed");
        assert!(j.rules.is_empty());
        assert_eq!(j.tools, vec!["read_file".to_string()]);
    }

    #[test]
    fn a_body_without_a_reply_or_a_model_is_refused_with_the_field() {
        let no_answer = judging_of(&json!({ "model": "m", "rules": [] })).unwrap_err();
        assert!(no_answer.contains("answer"), "{no_answer}");
        let no_model = judging_of(&json!({ "answer": "x", "rules": [] })).unwrap_err();
        assert!(no_model.contains("model"), "{no_model}");
        let bad_tools =
            judging_of(&json!({ "answer": "x", "model": "m", "tools": "read_file" })).unwrap_err();
        assert!(bad_tools.contains("tools"), "{bad_tools}");
        let blank_tool =
            judging_of(&json!({ "answer": "x", "model": "m", "tools": [""] })).unwrap_err();
        assert!(blank_tool.contains("tools"), "{blank_tool}");
    }

    #[test]
    fn a_long_reply_is_judged_on_its_head() {
        let long = "a".repeat(ANSWER_MAX + 50);
        let j =
            judging_of(&json!({ "answer": long, "model": "m", "rules": ["implied_completion"] }))
                .expect("well-formed");
        assert_eq!(j.answer.chars().count(), ANSWER_MAX);
    }
}

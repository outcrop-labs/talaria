// /api/muse.
//
// The Muse endpoint. ONE route, TWO answers, because the Muse does two
// genuinely different things and used to pretend they were one:
//
//   PROSE (soul, personality, skill, memory, document, template)
//     text/plain, streamed. Tokens landing in the editor as they arrive is the
//     feature, and nothing below changes it.
//
//   JSON (cron, agent, ticket, skillForm, templateForm)
//     application/json, VALIDATED HERE. These streamed too, once — the first
//     three of them (cron, agent, ticket) before the browser pulled the object
//     back out with a greedy `/\{[\s\S]*\}/` (audit 1.1 — the extractor
//     verified to fail on three shapes a 14B model emits constantly). The two
//     form kinds land here directly, because the record a view is standing in
//     is a contract a small model misses without a repair turn. On failure the
//     client gets a validated value or a sentence saying why not.
//
// THE STREAMING HALF IS A HARNESS TOO — through the runner's own streaming
// entry point, not around it (audit 1.5, the Muse row: these six draft SOULS,
// SKILLS and MEMORIES and ran with no guardrail at all). One run resolves the
// model, pumps the SSE frames through `gateway_stream`, meters the turn,
// guards the completed reply, and applies `onFailure`. What is left here is
// the part that is genuinely this route's: turning deltas into an HTTP body.
//
// What the runner does NOT do is redact what was RELAYED, because by then every
// character is on the wire. Strict-mode redaction happens on the way OUT, chunk
// by chunk, in the delta tap — `StreamRedactor`. guardrails says strict mode
// cleans "what Talaria persists or hasn't yet relayed"; on this path the
// accumulated stream IS the saved document, so those are one string and the
// only place to catch it is before it leaves.

use crate::gateway::guard::redact_secrets;
use crate::gateway::guard::{GuardMode, guard_config};
use crate::harness::define::HarnessDefinition;
use crate::harness::run::{
    DeltaFn, HarnessError, RunContext, StreamFn, StreamOptions, run_harness, run_harness_streamed,
};
use crate::harness::transport::gateway_stream;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use talaria_body::{
    array_too_big_msg, as_object, enum_member, optional_max_string_member, parse, string_msg,
    too_big_msg, zod_type_name,
};
use talaria_error::{house_error, thrown_internal_error};
use talaria_harness_defs::defs::muse::{
    MuseDraftInput, MuseProseInput, MuseProseKind, MuseTurn, muse_agent_harness, muse_cron_harness,
    muse_draft_harness, muse_skill_form_harness, muse_template_form_harness, muse_ticket_harness,
};
use talaria_harness_model::{MUSE_CHAIN, ModelSpec, ResolvedHarnessModel, resolve_harness_model};
use talaria_org::{org_line, org_profile};
use talaria_session::require_user;
use talaria_state::AppState;
use tokio::sync::{Notify, mpsc};

const KINDS: [&str; 11] = [
    "soul",
    "personality",
    "skill",
    "memory",
    "cron",
    "agent",
    "document",
    "template",
    "ticket",
    "skillForm",
    "templateForm",
];
const PROSE_KINDS: [&str; 6] = [
    "soul",
    "personality",
    "skill",
    "memory",
    "document",
    "template",
];

/// What the USER reads when the model could not hold the contract. Written per
/// kind and pointed at the next thing to try, because "the JSON could not be
/// parsed" is a fact about the model and not an instruction to a person. The
/// technical reason travels beside it as `detail`, and the full story — which
/// model, which chain step, how many repairs — is already on the harness_runs
/// row by the time this returns.
fn unusable(kind: &str) -> &'static str {
    match kind {
        "cron" => {
            "Muse could not turn that into a scheduled job — try saying when it should run and what it should do each time."
        }
        "agent" => {
            "Muse could not design an agent from that — try adding a sentence about what it should do."
        }
        "ticket" => {
            "Muse could not turn that into a ticket edit — try naming the fields to change."
        }
        "skillForm" => {
            "Muse could not fill out that skill — try saying what the skill does, one skill at a time."
        }
        _ => {
            "Muse could not fill out that template — try naming the template and a few sections it should have."
        }
    }
}

const NO_MODEL: &str = "no routable model found — add an endpoint with models on /models first";

/// How long a structured draft may buffer before its answer must open the
/// body — well inside Cloudflare's 100s idle ceiling, far past any honest
/// fast run — and then the gap between heartbeats.
const FIRST_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(45);
const HEARTBEAT_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// The structured kinds' ONE answer shape, shared by the buffered path and
/// the heartbeat path: `(status, body, x-muse-model)`. The buffered path
/// keeps the real statuses — 400 for a configuration problem, 502 for a
/// model problem, 500 for a run that threw; the heartbeat path's headers
/// are already committed to 200, so its failures ride in the body and the
/// client throws the `error` member either way.
#[allow(clippy::type_complexity)]
fn structured_answer(
    res: Result<Result<crate::harness::run::HarnessResult, HarnessError>, tokio::task::JoinError>,
    kind: &str,
) -> (StatusCode, String, Option<String>) {
    match res {
        Ok(Ok(r)) => {
            if let Some(value) = r.value {
                let body = json!({ "value": value, "model": r.model.clone() }).to_string();
                return (StatusCode::OK, body, r.model);
            }
            // No model at all is a CONFIGURATION problem and the admin needs
            // the real sentence; a model that answered badly is a MODEL
            // problem and the user needs something they can act on. Two
            // failures, two status codes.
            if r.model.is_none() {
                let error = r.error.unwrap_or_else(|| NO_MODEL.to_string());
                return (
                    StatusCode::BAD_REQUEST,
                    json!({ "error": error }).to_string(),
                    None,
                );
            }
            (
                StatusCode::BAD_GATEWAY,
                json!({ "error": unusable(kind), "detail": r.error }).to_string(),
                None,
            )
        }
        Ok(Err(HarnessError(e))) => {
            tracing::error!("[muse] {kind} run failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Internal Server Error" }).to_string(),
                None,
            )
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Internal Server Error" }).to_string(),
            None,
        ),
    }
}

/// The body's `chat` member: an optional array of prior turns, each
/// `{ role: 'user' | 'assistant', content: ≤300k }`, at most 24 of them.
fn parse_chat(obj: &serde_json::Map<String, Value>) -> Result<Option<Vec<MuseTurn>>, String> {
    let Some(v) = obj.get("chat") else {
        return Ok(None);
    };
    let Some(items) = v.as_array() else {
        return Err(array_msg(v));
    };
    if items.len() > 24 {
        return Err(array_too_big_msg(24));
    }
    let mut turns = Vec::with_capacity(items.len());
    for item in items {
        let Some(map) = item.as_object() else {
            return Err(talaria_body::object_msg(zod_type_name(item)));
        };
        let role = enum_member(map, "role", &["user", "assistant"])?;
        let content = match map.get("content") {
            None => return Err(string_msg("undefined")),
            Some(c) => c.as_str().ok_or_else(|| string_msg(zod_type_name(c)))?,
        };
        if content.chars().count() > 300_000 {
            return Err(too_big_msg(300_000));
        }
        turns.push(MuseTurn {
            role,
            content: content.to_string(),
        });
    }
    Ok(Some(turns))
}

fn array_msg(v: &Value) -> String {
    format!(
        "Invalid input: expected array, received {}",
        zod_type_name(v)
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let kind = match enum_member(obj, "kind", &KINDS) {
        Ok(k) => k,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // instruction: trimmed, then 1..8_000 (empty after trim is a 400).
    let instruction = match obj.get("instruction") {
        None => return house_error(StatusCode::BAD_REQUEST, &string_msg("undefined")),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)));
            let s = match s {
                Ok(s) => s,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return house_error(StatusCode::BAD_REQUEST, &talaria_body::too_small_msg(1));
            }
            if trimmed.chars().count() > 8_000 {
                return house_error(StatusCode::BAD_REQUEST, &too_big_msg(8_000));
            }
            trimmed.to_string()
        }
    };
    let current = match optional_max_string_member(obj, "current", 300_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let context = match optional_max_string_member(obj, "context", 2_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let chat = match parse_chat(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let caller = format!(
        "platform:muse:{}",
        user.email
            .as_deref()
            .or(user.name.as_deref())
            .unwrap_or(&user.id)
    );

    // The org anchor is DECORATION on the prompt, so a settings read that
    // fails must not take the draft down with it — None means no anchor.
    let org = org_line(&org_profile(&state.pg).await);

    // ── The structured kinds ────────────────────────────────────────────────
    if !PROSE_KINDS.contains(&kind.as_str()) {
        let input = serde_json::to_value(MuseDraftInput {
            instruction,
            current,
            context,
            chat,
            org: org.clone(),
        })
        .expect("the draft input serializes");
        // The model resolves BEFORE the run opens, for the same reason the
        // prose half resolves before its first byte: "nothing routes" must be
        // a 400 with an admin-readable sentence, not a 200 whose body says so
        // after the headers committed — and the heartbeat answer below wants
        // `x-muse-model` on headers it can no longer take back.
        let user_spec = ModelSpec {
            pin: Some("muse"),
            role: None,
            chain: Some(&MUSE_CHAIN),
            user_id: Some(&user.id),
        };
        let resolved: Option<ResolvedHarnessModel> =
            match resolve_harness_model(&state.pg, &user_spec).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("[muse] model resolution failed: {e}");
                    return thrown_internal_error();
                }
            };
        let Some(resolved) = resolved else {
            return house_error(StatusCode::BAD_REQUEST, NO_MODEL);
        };
        // `userId` is what arms the member model allowlist inside the chain
        // (see MUSE_MODEL) — a harness run without it would hand a member the
        // expensive model an admin gated. The pinned model hands the
        // pre-resolved answer over rather than asking the chain again.
        let ctx = RunContext {
            caller,
            user_id: Some(user.id),
            model: Some(resolved.model.clone()),
            step: Some(resolved.step),
            ..Default::default()
        };
        let run_state = state.clone();
        let run_kind = kind.clone();
        let mut done = tokio::spawn(async move {
            let def: HarnessDefinition = match run_kind.as_str() {
                "cron" => muse_cron_harness(),
                "agent" => muse_agent_harness(),
                "ticket" => muse_ticket_harness(),
                "skillForm" => muse_skill_form_harness(),
                _ => muse_template_form_harness(),
            };
            run_harness(&run_state, &def, &input, ctx).await
        });

        // Hold the answer the way the prose half holds its first token: a run
        // that ends inside FIRST_HEARTBEAT answers with today's buffered JSON,
        // statuses intact. A structured draft puts NOTHING on the wire until
        // the whole validated JSON exists, so a slow generation (a cold
        // provider minute is normal; the 2026-09-14 outcrop incident ran
        // 2m20s) otherwise runs head-first into every intermediary's idle
        // ceiling — Cloudflare's 100s first — and the browser gets a 524 the
        // api never sees. A run that outlives the threshold gets an
        // application/json body that drips `\n` (leading whitespace is legal
        // JSON; the client's parse cannot tell) and ends with the same object.
        tokio::select! {
            res = &mut done => {
                let (status, body, model) = structured_answer(res, &kind);
                let mut resp = Response::builder()
                    .status(status)
                    .header(header::CACHE_CONTROL, "no-cache")
                    .header(header::CONTENT_TYPE, "application/json");
                if let Some(m) = model.as_deref() {
                    resp = resp.header("x-muse-model", m);
                }
                return resp
                    .body(Body::from(body))
                    .expect("static headers build");
            }
            _ = tokio::time::sleep(FIRST_HEARTBEAT) => {}
        }

        let (btx, brx) = mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
        let pump_kind = kind.clone();
        let model_header = resolved.model.clone();
        tokio::spawn(async move {
            let mut next = tokio::time::Instant::now() + HEARTBEAT_EVERY;
            loop {
                tokio::select! {
                    res = &mut done => {
                        let (_, body, _) = structured_answer(res, &pump_kind);
                        let _ = btx.send(Ok(Bytes::from(body)));
                        // Dropping the sender closes the body stream.
                        drop(btx);
                        break;
                    }
                    _ = tokio::time::sleep_until(next) => {
                        let _ = btx.send(Ok(Bytes::from_static(b"\n")));
                        next += HEARTBEAT_EVERY;
                    }
                }
            }
        });
        let stream =
            futures_util::stream::unfold(
                brx,
                |mut rx| async move { rx.recv().await.map(|i| (i, rx)) },
            );
        // Header order on the wire is alphabetical — cache-control, then
        // content-type, then x-muse-model (the prose answer below is the
        // precedent for caring).
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-muse-model", &model_header)
            .body(Body::from_stream(stream))
            .expect("static headers build");
    }

    // ── The prose kinds: stream ─────────────────────────────────────────────
    let prose_kind = match kind.as_str() {
        "soul" => MuseProseKind::Soul,
        "personality" => MuseProseKind::Personality,
        "skill" => MuseProseKind::Skill,
        "memory" => MuseProseKind::Memory,
        "document" => MuseProseKind::Document,
        _ => MuseProseKind::Template,
    };
    let input_value = serde_json::to_value(MuseProseInput {
        kind: prose_kind,
        draft: MuseDraftInput {
            instruction,
            current,
            context,
            chat,
            org,
        },
    })
    .expect("the prose input serializes");

    // Resolved HERE and handed to the run as a fixed answer, for the two things
    // a header cannot get from a promise: `x-muse-model` has to be on the
    // Response before the first byte, and "nothing routes" has to be a 400 with
    // an admin-readable sentence rather than a stream that opens and closes
    // empty. `step` travels with it so the harness_runs row still records WHICH
    // chain step won — an install limping along on 'first-routable' is a real
    // finding, and pinning `RunContext.model` alone would erase it.
    let user_spec = ModelSpec {
        pin: Some("muse"),
        role: None,
        chain: Some(&MUSE_CHAIN),
        user_id: Some(&user.id),
    };
    let resolved: Option<ResolvedHarnessModel> =
        match resolve_harness_model(&state.pg, &user_spec).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[muse] model resolution failed: {e}");
                return thrown_internal_error();
            }
        };
    let Some(resolved) = resolved else {
        return house_error(StatusCode::BAD_REQUEST, NO_MODEL);
    };
    let model = resolved.model.clone();

    // Read BEFORE the stream opens, because strict mode has to redact on the way
    // out and there is no way back once a chunk is sent. A settings read that
    // fails must not take the draft down with it — the guard is off for this
    // call and the run below is still recorded.
    let strict = matches!(guard_config(&state.pg).await.mode, GuardMode::Strict);
    let redactor = strict.then(|| {
        Arc::new(Mutex::new(
            talaria_harness_defs::defs::muse::StreamRedactor::new(Arc::new(|text: &str| {
                redact_secrets(text, None).0
            })),
        ))
    });

    // Deltas out through an UNBOUNDED channel, enqueued without backpressure:
    // a draft is bounded by the model's own output length, and holding the
    // transport mid-stream to pace a browser would stall the guard pass and
    // the metering behind it. The reader going away (closed tab, cancelled
    // draft) is `gone`; enqueueing after that is a no-op rather than a failed
    // harness run.
    let (btx, brx) = mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
    let relayed = Arc::new(AtomicBool::new(false));
    // Notify rather than a oneshot: `send` consumes a oneshot Sender, and a
    // delta tap is an Fn — it fires per token, not once. Notify hands out a
    // stored permit, so a delta that lands before the select polls still
    // releases it.
    let opened = Arc::new(Notify::new());

    let on_delta: DeltaFn = {
        let relayed = relayed.clone();
        let redactor = redactor.clone();
        let btx = btx.clone();
        let opened = opened.clone();
        Arc::new(move |delta: &str| {
            relayed.store(true, Ordering::SeqCst);
            opened.notify_one();
            // Outside strict mode this is the identity — the stream passes
            // through unchanged. In strict mode the redactor holds back the
            // tail and cuts only where a secret pattern cannot straddle.
            let text = match redactor.as_ref() {
                Some(r) => r.lock().expect("the redactor is not contended").push(delta),
                None => delta.to_string(),
            };
            if !text.is_empty() {
                let _ = btx.send(Ok(Bytes::from(text)));
            }
        })
    };

    // ONE run, streamed. `gateway_stream` pumps the SSE frames and meters the
    // turn; the runner resolves nothing (pinned above), renders from the
    // definition, guards the completed reply with the harness's own rule set,
    // writes the findings and the harness_runs row, and applies `onFailure`.
    let stream_state = state.clone();
    let stream: StreamFn = Arc::new(move |req, emit| {
        let st = stream_state.clone();
        Box::pin(async move { gateway_stream(&st, &req, |s| emit(s)).await })
    });
    let ctx = RunContext {
        caller,
        // `userId` arms the member model allowlist inside the chain (see
        // MUSE_MODEL) — it is what stops a member being handed the expensive
        // model an admin gated. It travels even though the chain was already
        // answered above, because the run must be correct if the pin below is
        // ever dropped.
        user_id: Some(user.id),
        // The chain ran above (the header needs its answer before the first
        // byte), so this hands the ANSWER over rather than asking again.
        model: Some(resolved.model),
        step: Some(resolved.step),
        ..Default::default()
    };
    let run_state = state.clone();
    let btx_owner = btx;
    let redactor_end = redactor.clone();
    let mut done = tokio::spawn(async move {
        let def = muse_draft_harness();
        let res = run_harness_streamed(
            &run_state,
            &def,
            &input_value,
            ctx,
            StreamOptions {
                stream,
                on_delta: Some(on_delta),
            },
        )
        .await;
        // The held-back tail. Skipping this would truncate every strict-mode
        // draft by its last token.
        if let Some(r) = redactor_end.as_ref() {
            let tail = r.lock().expect("the redactor is not contended").flush();
            if !tail.is_empty() {
                let _ = btx_owner.send(Ok(Bytes::from(tail)));
            }
        }
        // Dropping the sender closes the body stream.
        drop(btx_owner);
        res
    });

    // Hold the Response until either the first token or the end of the run. A
    // run that ended without relaying anything never opened a stream, so it can
    // still be answered as an error — which is the difference between "the
    // gateway refused" and a 200 with an empty body that reads to the user as a
    // Muse that did nothing.
    tokio::select! {
        _ = opened.notified() => {}
        _ = &mut done => {}
    }
    if !relayed.load(Ordering::SeqCst) {
        // JoinError → the run task died (panic/cancel); HarnessError → the
        // run's own refusal. Either way there is no reply to quote, and the
        // model DID resolve (we pinned it), so this is the 502 half of the
        // same split the structured kinds make above.
        let res = done.await.ok();
        let error = res
            .and_then(|r| r.ok())
            .and_then(|r| r.error)
            .unwrap_or_else(|| "the model returned nothing".to_string());
        return house_error(StatusCode::BAD_GATEWAY, &error);
    }

    let stream =
        futures_util::stream::unfold(
            brx,
            |mut rx| async move { rx.recv().await.map(|i| (i, rx)) },
        );
    // Header order on the wire is alphabetical — cache-control, then
    // content-type, then x-muse-model.
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("x-muse-model", &model)
        .body(Body::from_stream(stream))
        .expect("static headers build")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::run::HarnessResult;

    fn result(
        value: Option<Value>,
        model: Option<&str>,
        error: Option<&str>,
    ) -> Result<HarnessResult, HarnessError> {
        Ok(HarnessResult {
            value,
            model: model.map(String::from),
            step: None,
            widened: false,
            repairs: 0,
            schema_valid: false,
            answered: false,
            refused: false,
            findings: Vec::new(),
            raw: None,
            latency_ms: 0,
            escalate: false,
            error: error.map(String::from),
        })
    }

    // The structured kinds' status split, pinned: a value is a 200 carrying
    // the model header, a run that never reached a model is a 400 with the
    // admin's sentence, a model that answered unusably is a 502 with the
    // user's sentence — the same three answers the buffered path always
    // gave, now shared with the heartbeat path where only the body can carry
    // them.
    #[test]
    fn the_structured_answer_keeps_the_status_split() {
        let (status, body, model) = structured_answer(
            Ok(result(Some(json!({"name": "Casey"})), Some("m"), None)),
            "agent",
        );
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"value":{"name":"Casey"},"model":"m"}"#);
        assert_eq!(model.as_deref(), Some("m"));

        let (status, body, model) = structured_answer(Ok(result(None, None, None)), "agent");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": NO_MODEL }).to_string());
        assert_eq!(model, None);

        let (status, body, model) =
            structured_answer(Ok(result(None, Some("m"), Some("shape"))), "agent");
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body,
            json!({ "error": unusable("agent"), "detail": "shape" }).to_string()
        );
        assert_eq!(model, None);

        let (status, _, model) = structured_answer(Ok(Err(HarnessError("boom".into()))), "agent");
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(model, None);
    }
}

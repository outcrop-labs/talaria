// /api/muse — port of ui/src/routes/api/muse.ts.
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

use crate::body::{
    array_too_big_msg, as_object, enum_member, optional_max_string_member, parse, string_msg,
    too_big_msg, zod_type_name,
};
use crate::error::{house_error, thrown_internal_error};
use crate::gateway::guard::redact_secrets;
use crate::gateway::guard::{GuardMode, guard_config};
use crate::harness::define::HarnessDefinition;
use crate::harness::defs::muse::{
    MuseDraftInput, MuseProseInput, MuseProseKind, MuseTurn, muse_agent_harness, muse_cron_harness,
    muse_draft_harness, muse_skill_form_harness, muse_template_form_harness, muse_ticket_harness,
};
use crate::harness::run::{
    DeltaFn, HarnessError, RunContext, StreamFn, StreamOptions, run_harness, run_harness_streamed,
};
use crate::harness::transport::gateway_stream;
use crate::harness_model::{MUSE_CHAIN, ModelSpec, ResolvedHarnessModel, resolve_harness_model};
use crate::org::{org_line, org_profile};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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
            return Err(crate::body::object_msg(zod_type_name(item)));
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
    // z.string().trim().min(1).max(8_000)
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
                return house_error(StatusCode::BAD_REQUEST, &crate::body::too_small_msg(1));
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
        // `userId` is what arms the member model allowlist inside the chain
        // (see MUSE_MODEL) — a harness run without it would hand a member the
        // expensive model an admin gated.
        let ctx = RunContext {
            caller,
            user_id: Some(user.id.clone()),
            ..Default::default()
        };
        let def: HarnessDefinition = match kind.as_str() {
            "cron" => muse_cron_harness(),
            "agent" => muse_agent_harness(),
            "ticket" => muse_ticket_harness(),
            "skillForm" => muse_skill_form_harness(),
            _ => muse_template_form_harness(),
        };
        let res = match run_harness(&state, &def, &input, ctx).await {
            Ok(r) => r,
            Err(HarnessError(e)) => {
                tracing::error!("[muse] {kind} run failed: {e}");
                return thrown_internal_error();
            }
        };
        if let Some(value) = res.value {
            let mut resp = Json(json!({ "value": value, "model": res.model })).into_response();
            resp.headers_mut().insert(
                "x-muse-model",
                axum::http::HeaderValue::from_str(res.model.as_deref().unwrap_or(""))
                    .unwrap_or(axum::http::HeaderValue::from_static("")),
            );
            return resp;
        }
        // No model at all is a CONFIGURATION problem and the admin needs the real
        // sentence; a model that answered badly is a MODEL problem and the user
        // needs something they can act on. Two failures, two status codes.
        if res.model.is_none() {
            let error = res.error.unwrap_or_else(|| NO_MODEL.to_string());
            return house_error(StatusCode::BAD_REQUEST, &error);
        }
        let mut resp =
            Json(json!({ "error": unusable(&kind), "detail": res.error })).into_response();
        *resp.status_mut() = StatusCode::BAD_GATEWAY;
        return resp;
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
        Arc::new(Mutex::new(crate::harness::defs::muse::StreamRedactor::new(
            Arc::new(|text: &str| redact_secrets(text, None).0),
        )))
    });

    // Deltas out through an UNBOUNDED channel, enqueued without backpressure —
    // the TS `push` deliberately does not wait on `desiredSize`: a draft is
    // bounded by the model's own output length, and holding the transport
    // mid-stream to pace a browser would stall the guard pass and the metering
    // behind it. The reader going away (closed tab, cancelled draft) is `gone`;
    // enqueueing after that is a no-op rather than a failed harness run.
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
            // Outside strict mode this is the identity and the stream is
            // byte-for-byte what it always was. In strict mode the redactor
            // holds back the tail and cuts only where a secret pattern cannot
            // straddle.
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
    // Header order matches the oracle's wire, not its source: TS builds the
    // response through a fetch Headers object, which iterates alphabetically.
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("x-muse-model", &model)
        .body(Body::from_stream(stream))
        .expect("static headers build")
}

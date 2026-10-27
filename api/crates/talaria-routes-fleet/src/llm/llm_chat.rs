// POST /api/llm/v1/chat/completions. OpenAI-compatible chat over the org's
// model stack: streaming and non-streaming both relay, every call metered
// into the ledger under the calling key's identity, every completion through
// the confab guard (gateway/guard.rs).
//
// The pipeline: bearer → authenticate_key → the key's own rpm brake → body
// validation → the agent key's model gate → resolve_route → check_budget
// (org + caller, key caps min-merged under the admin's ceiling) →
// build_upstream (defaults UNDER the
// client body, secret-vault seal, learned-param pre-strip) → fetch_upstream
// (400 strip-and-retry ≤4, dev hostname fallback) → relay. Non-streaming
// reads,
// meters, relays — annotate/strict rewriting the body before it goes out;
// streaming passes bytes through a metering Stream that scans SSE lines and
// settles the ledger exactly once from wherever the stream ends — clean
// flush, client hangup (Drop), or upstream error.
//
// WHO IS CALLING. An operator's own key is `api:<name>` everywhere — the
// budget subject, the ledger row, the guard finding. An AGENT'S key
// (`agent:<slug>`, minted per enabled agent by fleet-brain and stamped into
// that agent's container alone) is the agent: its subject is the agent's
// model id — the one `agent_model` every row of its spend lands under, and
// the name its budget binds — and it may ask only for the model ids its
// rendered config routes to (gateway_agent_models, written by the render);
// anything else is a 403 before any upstream is touched.
//
// EVERY AGENT CALL IS METERED HERE, one row per upstream call — this is the
// only writer of an agent's spend. Whose turn the call belongs to rides on
// it: Hermes stamps `x-talaria-session` (the rendered provider entry's
// `session_affinity_header`) with the conversation's key, and the row takes
// the flow the chat transport recorded for that key — chat, channel, ticket,
// research, with its conversation or run and ticket — or stays `unattended`
// (a cron's `cron_<job id>`, no header, a key nobody recorded). gateway
// usage.rs, "Attribution", has the whole mechanism.
//
// AN APP'S KEY (`app:<slug>`, minted per enabled app by talaria-apps and held
// by the app host alone — app_keys.rs) is the app: its budget subject is
// `app:<slug>`, every row it books lands under the run's caller stamp inside
// that namespace with source `app`, and the run's ref and ticket ride along
// when they are uuids. It is the app's only while the lifecycle records its
// id; a key merely NAMED like one is refused. Two things differ from any
// other key. A model the gateway does not serve but that is a live fleet
// agent (or a tier of one) is driven for one turn through the persona door
// (fleet_chat.rs), with the turn's attribution recorded as `app` spend, so
// the agent's own calls — on its own key, through this relay — are the one
// meter of that turn. And the relay runs no guard pass of its own: the app
// host's harness runner guards every reply with the harness's narrowed rule
// set, and a second pass here would file every finding twice. The runner
// runs the structural rules; the semantic ones need the decision model,
// which lives here, so the runner hands the run's final reply and tool
// record to `/api/internal/apps/{slug}/guard` once per run — never per call,
// because a call here cannot tell a run's answer from a repair or a loop
// turn, and cannot see a tool record the harness grounded itself. The answer
// says which side served it and which contract parameters never reached the
// model (`x-talaria-transport`, `x-talaria-contract-drops`).
//
// THE GUARD'S TWO POSTURES. Observe (or an agent-loop key): the guard runs
// detached after the bytes are gone — findings record, response untouched.
// Annotate/strict on a human's key: non-streaming awaits the guard and
// rewrites the body (strict redacts, both append the caveat); streaming
// relays line-wise with [DONE] withheld, then injects the caveat as one
// final delta chunk. AN AGENT'S OWN TOOL LOOP IS NEVER ANNOTATED — a caveat
// mid-loop would contaminate the agent's context; the chat/channel layer
// annotates the human-facing copy instead.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use futures_util::Stream;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use talaria_api_facades::decide::guard as decide_guard;
use talaria_api_facades::fleet::list_agents;
use talaria_api_facades::gateway::agent_keys::{
    agent_key_scope, agent_slug_of_key, model_in_scope,
};
use talaria_api_facades::gateway::app_keys::{
    APP_SOURCE, CALLER_HEADER, CANNED_HEADER, CONTRACT_DROPS_HEADER, HOLD_HEADER, REF_HEADER,
    TASK_HEADER, TRANSPORT_HEADER, app_caller_stamp, app_key_is_current, app_slug_of_key,
    contract_drops_value, hold_ms_of, persona_base,
};
use talaria_api_facades::gateway::budget::BudgetDenial;
use talaria_api_facades::gateway::budget::{BudgetLimits, budget_message, check_budget};
use talaria_api_facades::gateway::fleet_chat::proxy_chat;
use talaria_api_facades::gateway::guard::{
    Finding, Grounding, GuardMode, extract_tool_record, grounding_text_of, guard_completion,
    guard_config, needs_redaction, redact_secrets,
};
use talaria_api_facades::gateway::registry::resolve_route;
use talaria_api_facades::gateway::settings::get_setting_hot;
use talaria_api_facades::gateway::upstream::{Reply, build_upstream, fetch_upstream, js_truthy};
use talaria_api_facades::gateway::usage::{
    RELAY_SOURCE, ReportedSpend, SESSION_HEADER, StatedFlow, TokenCounts, TurnAttribution,
    UNATTENDED_SOURCE, affinity_key, estimate_tokens, normalize_usage, record_relay_usage,
    reported_spend,
};
use talaria_auth::{KeyIdentity, authenticate_key, bearer_secret};
use talaria_error::{
    BudgetFacts, internal, js_num, log_upstream_error, openai_budget_error, openai_error,
    openai_error_null_param, sanitized_upstream_body,
};
use talaria_ratelimit::rate_limit;
use talaria_state::AppState;

/// 25MB is a sanity ceiling for a chat body — a body past it lands in the
/// same 400 as unparseable JSON.
const BODY_LIMIT: usize = 25 * 1024 * 1024;

// The two fleet-wide Talaria-minted gateway credentials (fleet-brain owns
// both). Named here only as the DEFAULTS for the settings below; operators
// can extend the lists, and any other key — an operator's own — is metered
// and annotated. The per-agent keys (`agent:<slug>`) are recognised by
// prefix, never listed: a roster that changes every render is not a setting.
const PERSONA_KEY: &str = "fleet-gateway";
const WORKBENCH_KEY: &str = "workbench-gateway";

/// The subject a key's calls are budgeted, ledgered and guarded under: an
/// agent's key answers with the agent's model id (the agent's ledger
/// namespace); an app's with its own name, `app:<slug>` (its rows land under
/// the run's caller stamp inside it — `AppCall`); any other key with
/// `api:<name>`. The agent's slug stands in
/// for a key the last render did not scope — a disabled agent's, or one
/// rendered before scopes existed — so its spend still has a name.
///
/// That fallback name is not the one the chat transport records a turn's
/// flow under (the agent's model id), so an unscoped key's rows never join
/// an attribution: they are booked, under the slug, and stay `unattended`.
/// The next render scopes every enabled agent's key, which closes it.
fn caller_of(key_name: &str, scope_model: Option<&str>) -> String {
    match (agent_slug_of_key(key_name), scope_model) {
        (Some(_), Some(model)) => model.to_string(),
        (Some(slug), None) => slug.to_string(),
        (None, _) if app_slug_of_key(key_name).is_some() => key_name.to_string(),
        (None, _) => format!("api:{key_name}"),
    }
}

/// What an app's call says about its run, read off the headers the app host
/// sets — honored for an app's key alone (any other key's are ignored).
struct AppCall {
    /// The name this call's rows land under: the run's caller stamp, kept
    /// inside the app's namespace.
    stamp: String,
    /// The run's ref and ticket, booked when they are uuids.
    stated: StatedFlow,
    /// How long an agent turn may hold for a restarting agent.
    hold_ms: Option<u64>,
}

impl AppCall {
    fn read(slug: &str, headers: &axum::http::HeaderMap) -> Self {
        let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        let id = |name: &str| {
            h(name)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(String::from)
        };
        AppCall {
            stamp: app_caller_stamp(slug, h(CALLER_HEADER)),
            stated: StatedFlow {
                ref_id: id(REF_HEADER),
                task_id: id(TASK_HEADER),
            },
            hold_ms: hold_ms_of(h(HOLD_HEADER)),
        }
    }
}

/// The 429 a budget denial answers with — the same body and Retry-After
/// whichever door the call was headed for.
fn budget_refusal(denial: &BudgetDenial) -> Response {
    let mut out = openai_budget_error(
        StatusCode::TOO_MANY_REQUESTS,
        &budget_message(denial),
        "budget_exceeded",
        "budget_exceeded",
        BudgetFacts {
            scope: denial.scope.to_string(),
            subject: denial.subject.clone(),
            unit: denial.unit.to_string(),
            limit: js_num(denial.limit),
            used: js_num(denial.used),
            window_hours: denial.window_hours,
            via: denial.via.to_string(),
        },
    );
    out.headers_mut().insert(
        header::RETRY_AFTER,
        header::HeaderValue::from_str(&denial.retry_after_seconds.max(1).to_string())
            .unwrap_or(header::HeaderValue::from_static("300")),
    );
    out
}

/// An app's answer from the gateway side: which side served it, and the
/// contract parameters that never reached the model, so the host's runner
/// can stop trusting a JSON mode the endpoint dropped and refuse a tool turn
/// whose tools were stripped.
fn app_gateway_answer(mut out: Response, drops: Option<&str>) -> Response {
    out.headers_mut().insert(
        TRANSPORT_HEADER,
        header::HeaderValue::from_static("gateway"),
    );
    if let Some(v) = drops.and_then(|d| header::HeaderValue::from_str(d).ok()) {
        out.headers_mut().insert(CONTRACT_DROPS_HEADER, v);
    }
    out
}

/// ONE TURN OF A LIVE FLEET AGENT, DRIVEN BY AN APP — the persona door
/// (`proxy_chat`) with the turn's attribution recorded as the app's: every
/// call the agent makes for the turn reaches this relay on the agent's own
/// key and is booked there, once, as `app` spend with the run's ref and
/// ticket. Nothing is booked here — the turn's spend is the agent's calls.
/// The app's budget still gates the turn: an app over its ceiling drives no
/// agent either. The agent's stream relays untouched; a canned sentence
/// (the agent never came back, or the fleet is unrendered) says so in
/// `x-talaria-canned`, so the host fails the run instead of reading the
/// sentence as the agent's answer.
async fn app_agent_turn(
    state: &AppState,
    id: &KeyIdentity,
    app: &AppCall,
    agent: String,
    payload: Value,
) -> Response {
    let caller = caller_of(&id.key_name, None);
    if let Some(denial) = check_budget(
        &state.pg,
        &caller,
        BudgetLimits {
            tokens: id.caps.tokens,
            usd: id.caps.usd,
        },
    )
    .await
    {
        return budget_refusal(&denial);
    }
    let attribution = TurnAttribution {
        pg: state.pg.clone(),
        agent_model: agent,
        source: APP_SOURCE.to_string(),
        ref_id: app.stated.ref_id.clone(),
        task_id: app.stated.task_id.clone(),
    };
    let stream = proxy_chat(&payload, app.hold_ms, Some(attribution)).await;
    let mut out = Response::builder()
        .status(StatusCode::from_u16(stream.status).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, stream.content_type.as_str())
        .header(header::CACHE_CONTROL, "no-cache")
        .header(TRANSPORT_HEADER, "fleet");
    if let Some(canned) = stream.canned {
        out = out.header(CANNED_HEADER, canned);
    }
    out.body(Body::from_stream(stream.body))
        .unwrap_or_else(|_| fixed_json(502, &sanitized_upstream_body(502, "")))
}

pub async fn post(State(state): State<AppState>, req: Request<Body>) -> Response {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let identity = match bearer_secret(auth_header) {
        Some(secret) => authenticate_key(&state.pg, secret).await,
        None => Ok(None), // no header: not even a lookup
    };
    let id = match identity {
        Ok(Some(id)) => id,
        Ok(None) => return openai_error(StatusCode::UNAUTHORIZED, "invalid API key"),
        Err(e) => return internal("[llm/v1/chat] key lookup failed", e),
    };
    // AN APP'S KEY IS THE APP'S ONLY WHILE THE LIFECYCLE RECORDS IT
    // (app_keys.rs): a key that is merely named `app:<slug>` — minted by
    // hand, or one a rotation replaced — answers like an unknown key.
    let app_call = match app_slug_of_key(&id.key_name) {
        Some(slug) if app_key_is_current(&state.pg, slug, &id.key_id).await => {
            Some(AppCall::read(slug, req.headers()))
        }
        Some(slug) => {
            tracing::warn!(
                "[llm/v1/chat] {} is not the key app \"{slug}\" holds — refused",
                id.key_name
            );
            return openai_error(StatusCode::UNAUTHORIZED, "invalid API key");
        }
        None => None,
    };
    // The conversation key Hermes stamped on the call, read before the body
    // is consumed; only an agent's key gets to use it (below).
    let session_header = affinity_key(
        req.headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok()),
    );

    // Detached last_used_at — same fire-and-forget write as the models route.
    {
        let pg = state.pg.clone();
        let key_id = id.key_id.clone();
        tokio::spawn(async move {
            if let Err(e) =
                sqlx::query("update llm_api_keys set last_used_at = now() where id::text = $1")
                    .bind(&key_id)
                    .execute(&pg)
                    .await
            {
                tracing::warn!("[llm/v1/chat] last_used_at update failed for {key_id}: {e}");
            }
        });
    }

    // The key's own request-rate ceiling (#265) — the cheapest refusal, first.
    // Unset (the default) or 0 skips it; a limiter outage fails open.
    if let Some(rpm) = id.caps.rpm.filter(|v| v.is_finite() && *v != 0.0)
        && let Ok(mut conn) = state.redis().await
    {
        let r = rate_limit(
            &mut conn,
            &format!("gw:key:{}", id.key_id),
            rpm.round() as i64,
            60,
        )
        .await;
        if !r.ok {
            let mut out = openai_error_null_param(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("rate limit exceeded for this key: {rpm} requests per minute"),
                "rate_limit_exceeded",
                "rate_limit_exceeded",
            );
            out.headers_mut().insert(
                header::RETRY_AFTER,
                header::HeaderValue::from_str(&r.retry_after.max(1).to_string())
                    .unwrap_or(header::HeaderValue::from_static("60")),
            );
            return out;
        }
    }

    // Any unreadable/unparseable body is the same 400 — both collapse into
    // one path.
    let bytes = match axum::body::to_bytes(req.into_body(), BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => return openai_error(StatusCode::BAD_REQUEST, "model and messages are required"),
    };
    let client_body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return openai_error(StatusCode::BAD_REQUEST, "model and messages are required"),
    };
    if !client_body["model"].is_string() || !client_body["messages"].is_array() {
        return openai_error(StatusCode::BAD_REQUEST, "model and messages are required");
    }
    let model = client_body["model"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // THE AGENT KEY'S MODEL GATE — before the route is even resolved. The
    // render scoped this key to the ids its config.yaml routes to; a request
    // for anything else did not come from that config. A key the render
    // never scoped is judged by nothing (there is no set to judge against)
    // and keeps the fleet-wide behaviour it had.
    let agent_slug = agent_slug_of_key(&id.key_name);
    let agent_scope = match agent_slug {
        Some(slug) => agent_key_scope(&state.pg, slug).await,
        None => None,
    };
    if let Some(scope) = &agent_scope
        && !model_in_scope(scope, &model)
    {
        tracing::warn!(
            "[llm/v1/chat] {} asked for \"{model}\" outside its rendered set {:?}",
            id.key_name,
            scope.models
        );
        return openai_error_null_param(
            StatusCode::FORBIDDEN,
            &format!(
                "model \"{model}\" is not in this agent's rendered configuration — its tiers are {}",
                if scope.models.is_empty() {
                    "none".to_string()
                } else {
                    scope.models.join(", ")
                }
            ),
            "permission_denied",
            "model_not_allowed",
        );
    }
    let is_agent_key = agent_slug.is_some();

    let route = match resolve_route(&state.pg, &model).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            // An app may drive a live fleet agent for one turn — the
            // gateway wins when a model is both, as the transport rule says.
            if let Some(app) = &app_call {
                let agents: Vec<String> = list_agents().await.into_iter().map(|a| a.id).collect();
                if let Some(agent) = persona_base(&model, &agents) {
                    return app_agent_turn(&state, &id, app, agent.to_string(), client_body).await;
                }
            }
            return openai_error(
                StatusCode::NOT_FOUND,
                &format!("unknown model \"{model}\" — GET /v1/models"),
            );
        }
        Err(e) => return internal("[llm/v1/chat] route resolve failed", e),
    };

    // The ceiling, BEFORE anything is spent upstream. Off unless an admin
    // configured a budget or the key set its own cap; the key's caps ride
    // min-merged — an owner can throttle their own key, never out-spend an
    // admin's ceiling. An agent's subject is its model id, so `perAgent` and
    // `agents{<model>}` bind it, and the window it is judged on is the one
    // its chat turns already land in.
    let caller = caller_of(&id.key_name, agent_scope.as_ref().map(|s| s.model.as_str()));
    if let Some(denial) = check_budget(
        &state.pg,
        &caller,
        BudgetLimits {
            tokens: id.caps.tokens,
            usd: id.caps.usd,
        },
    )
    .await
    {
        return budget_refusal(&denial);
    }

    let mut call = match build_upstream(&state, &route, &client_body).await {
        Ok(c) => c,
        Err(msg) => {
            // The build can fail on endpoint/credential internals — the caller
            // (an external key holder) gets a fixed sentence, the log gets why.
            log_upstream_error("build", "no-route", &msg);
            return openai_error(
                StatusCode::BAD_GATEWAY,
                "upstream request could not be built",
            );
        }
    };

    // METERING — every call is booked here, an agent's included: no other
    // writer books an agent's spend any more. The one way out is an
    // operator listing a key in `gateway_unmetered_keys` (empty by default),
    // and a key listed there is booked nowhere at all. An agent's row starts
    // `unattended` and keeps the conversation key Hermes stamped on the
    // call; the insert itself takes the flow recorded for that key, when
    // there is one. Another key's header is ignored — an operator's rows are
    // the operator's.
    let unmetered = get_setting_hot(&state.pg, "gateway_unmetered_keys", json!([])).await;
    let listed_unmetered = unmetered
        .as_array()
        .map(|a| a.iter().any(|v| v.as_str() == Some(id.key_name.as_str())))
        .unwrap_or(false);
    let meter = meter_plan(is_agent_key, app_call.is_some(), listed_unmetered);
    let affinity = session_header.filter(|_| honors_affinity(is_agent_key, &id.key_name));

    // ANNOTATION — is the caller an agent's own tool loop? Guard caveats are
    // for humans reading a reply; injecting one into an agent's loop would
    // contaminate its context, so annotate/strict never rewrite these keys'
    // responses (findings still record, and the chat/channel layer annotates
    // the human-facing copy). Both the personas' key and the workbench's are
    // loops.
    let agent_loop_keys = get_setting_hot(
        &state.pg,
        "gateway_agent_loop_keys",
        json!([PERSONA_KEY, WORKBENCH_KEY]),
    )
    .await;
    let is_agent_loop = is_agent_key
        || agent_loop_keys
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str() == Some(id.key_name.as_str())))
            .unwrap_or(false);
    // An app's call is guarded by the host's runner, never here — its
    // semantic rules too, through the internal guard door, once per run.
    let guarded = app_call.is_none();
    let may_annotate = guarded && !is_agent_loop && guard_config(&state.pg).await.mode.discloses();

    // Everything a guard call needs, held once for both relay paths.
    let guard_spec = GuardSpec {
        pg: state.pg.clone(),
        caller: caller.clone(),
        model: model.clone(),
        endpoint: route.endpoint.name.clone(),
        messages: client_body["messages"].clone(),
        enabled: guarded,
    };

    let prompt_chars = prompt_chars_of(&client_body["messages"]);

    let reply = match fetch_upstream(&state.pg, &mut call, Some(&route)).await {
        Ok(r) => r,
        Err(msg) => {
            // The fetch error can name the endpoint host — endpoint topology
            // is not a key holder's business. Fixed sentence out.
            log_upstream_error("fetch", "unreachable", &msg);
            return openai_error(StatusCode::BAD_GATEWAY, "upstream unreachable");
        }
    };

    // What the answer tells an app about the call, read now that the fetch
    // has settled every drop it is going to make.
    let app_drops = app_call
        .as_ref()
        .and_then(|_| contract_drops_value(call.contract_drops.iter().map(|d| d.capability)));

    let ledger = Ledger {
        pg: state.pg.clone(),
        // An app's rows land under the run's stamp; its budget above is the
        // app's.
        caller: app_call
            .as_ref()
            .map_or_else(|| caller.clone(), |a| a.stamp.clone()),
        endpoint: route.endpoint.name.clone(),
        endpoint_class: route.endpoint.class.clone(),
        provider: route.endpoint.provider.clone(),
        upstream_model: route.upstream_model.clone(),
        prompt_chars,
        skip: meter.skip(),
        source: meter.source(),
        affinity,
        stated: app_call
            .as_ref()
            .map(|a| a.stated.clone())
            .unwrap_or_default(),
    };
    let answer = |out: Response| {
        if app_call.is_some() {
            app_gateway_answer(out, app_drops.as_deref())
        } else {
            out
        }
    };

    // ── Non-streaming: read, meter, relay ─────────────────────────────────
    if !js_truthy(&call.body["stream"]) {
        let status = reply.status();
        let content_type = reply.content_type();
        let mut text = reply.text().await;
        if !(200..300).contains(&status) {
            // The upstream's own error body never crosses to the key holder;
            // verbatim goes to the log. Metering is not gated on ok: a
            // rejected call still reports (and is still billed for) its
            // usage. One without usage books NOTHING — the estimate fallback
            // must not run here, or a rejection would invent spend.
            log_upstream_error("llm-v1", status, &text);
            if let Ok(j) = serde_json::from_str::<Value>(&text)
                && let Some(reported) = normalize_usage(j.get("usage"))
            {
                let spend = reported_spend(&ledger.provider, &j);
                ledger.spawn(reported, false, &spend);
            }
            return answer(fixed_json(status, &sanitized_upstream_body(status, &text)));
        }
        if let Ok(mut j) = serde_json::from_str::<Value>(&text) {
            let content = j["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let counts = normalize_usage(j.get("usage"));
            let spend = reported_spend(&ledger.provider, &j);
            ledger.maybe_spawn(counts, content.encode_utf16().count(), &spend);
            if may_annotate && !content.is_empty() {
                // Nothing has been relayed yet, so annotate/strict can act on
                // the body itself: strict redacts leaked secrets, and any
                // findings append the human-facing caveat.
                let (findings, caveat, mode) = guard_completion(
                    &state.pg,
                    &content,
                    &client_body["messages"],
                    &caller,
                    &model,
                    Some(&route.endpoint.name),
                )
                .await;
                // THE SEMANTIC PASS, and it is deliberately after the
                // structural one and detached. `guard_completion` has already
                // decided the caveat and the redaction for this reply, so
                // nothing below can change what the caller receives — which
                // is what makes a judged rule safe to enable before anybody
                // knows its false-positive rate. It is a no-op unless an
                // operator has both configured a decision model and turned a
                // semantic rule on; both are off by default.
                let tool_names =
                    extract_tool_record(client_body["messages"].as_array().unwrap_or(&Vec::new()))
                        .backing_tools;
                decide_guard::observe(&state, &content, &tool_names, &caller, &model);
                if !findings.is_empty() && j["choices"][0]["message"].is_object() {
                    // GROUNDED, like the findings above: redaction gets the
                    // same grounding material the findings got, or the two
                    // halves of one rule disagree about the same span.
                    let safe = if mode == GuardMode::Strict && needs_redaction(&findings) {
                        redact_secrets(
                            &content,
                            Some(&Grounding::new(&grounding_text_of(
                                client_body["messages"]
                                    .as_array()
                                    .map(Vec::as_slice)
                                    .unwrap_or_default(),
                            ))),
                        )
                        .0
                    } else {
                        content.clone()
                    };
                    j["choices"][0]["message"]["content"] = json!(format!("{safe}{caveat}"));
                    if let Ok(rewritten) = serde_json::to_string(&j) {
                        text = rewritten;
                    }
                }
            } else {
                // Observe posture (or no content): findings record, response
                // bytes are already relay-ready and stay untouched.
                guard_spec.observe(&content);
            }
        }
        return answer(fixed_json_with_type(status, &text, &content_type));
    }

    // ── Streaming: pass bytes through, scan SSE lines for usage/content ──
    if !reply.is_ok() {
        // Same boundary as the non-streaming relay: a failed hop's body is
        // the upstream's words, not ours to forward. The client asked to
        // stream, but an error is one small JSON body — every OpenAI SDK
        // accepts that.
        let status = reply.status();
        let text = reply.text().await;
        log_upstream_error("llm-v1-stream", status, &text);
        return answer(fixed_json(status, &sanitized_upstream_body(status, &text)));
    }
    let Reply::Live(res) = reply else {
        return fixed_json(502, &sanitized_upstream_body(502, ""));
    };
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    let mode = if may_annotate {
        MeterMode::Annotate
    } else {
        MeterMode::Passthrough
    };
    let stream = MeteredStream::new(res.bytes_stream(), ledger, mode, guard_spec);
    answer(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(stream))
            .expect("static headers build"),
    )
}

/// `Response` with a raw string body — used where the body is already the
/// exact bytes to relay (sanitized upstream errors, verbatim successes).
fn fixed_json(status: u16, body: &str) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static response builds")
}

fn fixed_json_with_type(status: u16, body: &str, content_type: &str) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_string()))
        .expect("static response builds")
}

/// The prompt-size figure: string content counts its UTF-16 length, anything
/// else counts its JSON serialization (null and absent both serialize as
/// `""`). Key order in that serialization is alphabetical, so the non-string
/// arm can wander a few chars on exotic bodies; it feeds only a chars/4
/// estimate.
fn prompt_chars_of(messages: &Value) -> usize {
    let Some(list) = messages.as_array() else {
        return 0;
    };
    list.iter()
        .map(|m| match &m["content"] {
            Value::String(s) => s.encode_utf16().count(),
            Value::Null => 2, // JSON.stringify('') === '""'
            v => serde_json::to_string(v)
                .map(|s| s.encode_utf16().count())
                .unwrap_or(2),
        })
        .sum()
}

/// What the ledger does with one call, decided before the upstream is
/// touched: skip, because an operator listed the key unmetered, or book it
/// here under a source that says whose spend it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meter {
    /// An operator listed this key in `gateway_unmetered_keys`.
    Skip,
    /// Book it here, under this source.
    Book(&'static str),
}

impl Meter {
    fn skip(self) -> bool {
        self == Meter::Skip
    }

    fn source(self) -> &'static str {
        match self {
            Meter::Book(source) => source,
            Meter::Skip => RELAY_SOURCE,
        }
    }
}

/// Whose `x-talaria-session` header the relay books by. An agent's own key:
/// the conversation key Hermes stamps. The workbench key: a workbench job's
/// eval session (`talaria-app eval --gateway`), so a harness fixture replayed
/// in the sandbox is booked to the job's ticket. The workbench key is
/// fleet-wide, but the session key it can name is a random one prepare_env
/// recorded for one job and wrote into that job's workdir alone; a key no
/// flow recorded books nothing differently. Any other key is an operator's,
/// and its rows are the operator's.
fn honors_affinity(is_agent_key: bool, key_name: &str) -> bool {
    is_agent_key || key_name == WORKBENCH_KEY
}

/// The metering decision. An agent's call is booked under the agent as
/// `unattended` until its conversation key says which flow it belongs to
/// (the insert reads that); an app's is the app's, `app`; any other key's is
/// the key's own, `gateway`.
fn meter_plan(is_agent_key: bool, is_app_key: bool, listed_unmetered: bool) -> Meter {
    if listed_unmetered {
        return Meter::Skip;
    }
    Meter::Book(if is_agent_key {
        UNATTENDED_SOURCE
    } else if is_app_key {
        APP_SOURCE
    } else {
        RELAY_SOURCE
    })
}

/// The ledger write for one call — spawned, never awaited on the request
/// path.
#[derive(Clone)]
struct Ledger {
    pg: sqlx::PgPool,
    caller: String,
    endpoint: String,
    endpoint_class: String,
    provider: String,
    upstream_model: String,
    prompt_chars: usize,
    skip: bool,
    source: &'static str,
    /// The conversation key Hermes stamped on an agent's call — None for
    /// any other key, and for an agent call outside any conversation.
    affinity: Option<String>,
    /// The ref and ticket an app's run states for its rows — empty for
    /// every other key.
    stated: StatedFlow,
}

impl Ledger {
    /// Known usage books as reported; absent usage falls back to the chars/4
    /// estimate over prompt + completion. Provider cost rides along when the
    /// body had one.
    fn maybe_spawn(
        &self,
        counts: Option<TokenCounts>,
        content_chars: usize,
        spend: &ReportedSpend,
    ) {
        if self.skip {
            return;
        }
        let (counts, estimated) = match counts {
            Some(c) => (c, false),
            None => (
                TokenCounts {
                    prompt_tokens: estimate_tokens(self.prompt_chars),
                    completion_tokens: estimate_tokens(content_chars),
                    ..Default::default()
                },
                true,
            ),
        };
        self.spawn(counts, estimated, spend);
    }

    fn spawn(&self, counts: TokenCounts, estimated: bool, spend: &ReportedSpend) {
        if self.skip {
            return;
        }
        let l = self.clone();
        let spend = spend.clone();
        tokio::spawn(async move {
            if let Err(e) = record_relay_usage(
                &l.pg,
                l.source,
                &l.caller,
                l.affinity.as_deref(),
                &l.stated,
                &l.endpoint,
                &l.endpoint_class,
                &l.upstream_model,
                &counts,
                estimated,
                Some(&spend),
            )
            .await
            {
                tracing::warn!("[llm/v1/chat] ledger write failed: {e}");
            }
        });
    }
}

/// Everything one guard call needs — held by the meter so the stream's end,
/// wherever it lands, can run the guard with the same context the handler
/// would.
#[derive(Clone)]
struct GuardSpec {
    pg: sqlx::PgPool,
    caller: String,
    model: String,
    endpoint: String,
    messages: Value,
    /// False for an app's call: the host's runner guards it.
    enabled: bool,
}

impl GuardSpec {
    fn call(&self, answer: &str) -> GuardFut {
        let s = self.clone();
        let answer = answer.to_string();
        Box::pin(async move {
            guard_completion(
                &s.pg,
                &answer,
                &s.messages,
                &s.caller,
                &s.model,
                Some(&s.endpoint),
            )
            .await
        })
    }

    /// Observe posture: fire-and-forget, findings record, nobody is told.
    /// `try_current` because Drop can run outside the runtime at shutdown —
    /// losing the guard there is acceptable, panicking is not.
    fn observe(&self, answer: &str) {
        if !self.enabled {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let fut = self.call(answer);
            handle.spawn(async move {
                let _ = fut.await;
            });
        }
    }
}

type GuardFut = Pin<Box<dyn Future<Output = (Vec<Finding>, String, GuardMode)> + Send>>;

/// Which relay the stream is. Passthrough relays bytes untouched (observe
/// posture or an agent-loop key); annotate relays line-wise with [DONE]
/// withheld so a caveat can land before it.
#[derive(PartialEq, Clone, Copy)]
enum MeterMode {
    Passthrough,
    Annotate,
}

/// The meter: SSE `data:` lines are scanned for usage/content while the bytes
/// relay, and the ledger settles once, from wherever the stream ends — a
/// clean flush, a client hangup (this struct's Drop), or an upstream error
/// frame. The provider bills a hung-up stream exactly the same, so the ledger
/// books it too — exactly once, whichever fires first. Only the CLEAN end
/// scans the pending tail; settle books, it does not scan.
struct MeteredStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    buf: Vec<u8>,
    content: String,
    usage: Option<Value>,
    generation_id: Option<String>,
    settled: bool,
    ledger: Ledger,
    mode: MeterMode,
    saw_done: bool,
    out: VecDeque<Bytes>,
    guard: Option<GuardFut>,
    ended: bool,
    errored: bool,
    guard_spec: GuardSpec,
}

/// A data: line whose payload is exactly [DONE].
fn line_is_done(line: &[u8]) -> bool {
    line.strip_prefix(b"data:")
        .is_some_and(|rest| String::from_utf8_lossy(rest).trim() == "[DONE]")
}

/// The caveat as one final delta chunk — the byte shape is the contract
/// (preserve_order keeps the key order; pinned byte-for-byte below).
fn guard_caveat_chunk(caveat: &str, model: &str) -> Bytes {
    let chunk = json!({
        "id": "talaria-guard",
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{ "index": 0, "delta": { "content": caveat }, "finish_reason": null }],
    });
    Bytes::from(format!("data: {chunk}\n\n"))
}

impl MeteredStream {
    fn new(
        inner: impl Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
        ledger: Ledger,
        mode: MeterMode,
        guard_spec: GuardSpec,
    ) -> Self {
        MeteredStream {
            inner: Box::pin(inner),
            buf: Vec::new(),
            content: String::new(),
            usage: None,
            generation_id: None,
            settled: false,
            ledger,
            mode,
            saw_done: false,
            out: VecDeque::new(),
            guard: None,
            ended: false,
            errored: false,
            guard_spec,
        }
    }

    fn scan_line(&mut self, line: &str) {
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data == "[DONE]" {
            return;
        }
        let Ok(j) = serde_json::from_str::<Value>(data) else {
            return;
        }; // partial line — ignore
        if j.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
            self.usage = j.get("usage").cloned();
        }
        if let Some(id) = j.get("id").and_then(Value::as_str)
            && (id.starts_with("gen-") || self.ledger.provider == "openrouter")
        {
            self.generation_id = Some(id.to_string());
        }
        if let Some(delta) = j["choices"][0]["delta"]["content"].as_str() {
            self.content.push_str(delta);
        }
    }

    fn on_chunk(&mut self, chunk: &Bytes) {
        self.buf.extend_from_slice(chunk);
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            // The drained line carries its own \n (and any \r before it);
            // passthrough ignores it, annotate re-emits exactly those bytes.
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let body = &line[..line.len() - 1];
            self.scan_line(&String::from_utf8_lossy(body));
            if self.mode == MeterMode::Annotate {
                if line_is_done(body) {
                    self.saw_done = true;
                } else {
                    self.out.push_back(Bytes::from(line));
                }
            }
        }
    }

    /// The clean-end tail: scanned, and in annotate mode re-emitted verbatim
    /// — no newline added.
    fn flush_tail(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.buf);
        self.scan_line(&String::from_utf8_lossy(&tail));
        if self.mode == MeterMode::Annotate {
            if line_is_done(&tail) {
                self.saw_done = true;
            } else {
                self.out.push_back(Bytes::from(tail));
            }
        }
    }

    /// Book the ledger — once, from whichever end the stream comes to. The
    /// pending line is NOT scanned here: settle books only, and a cancelled
    /// or errored stream's partial tail was never a complete frame.
    fn settle(&mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        let spend = match &self.usage {
            Some(usage) => {
                let mut body = serde_json::json!({ "usage": usage });
                if let Some(id) = &self.generation_id {
                    body["id"] = serde_json::Value::String(id.clone());
                }
                reported_spend(&self.ledger.provider, &body)
            }
            None => ReportedSpend {
                generation_id: self.generation_id.clone(),
                provider: Some(self.ledger.provider.clone()),
                ..ReportedSpend::default()
            },
        };
        self.ledger.maybe_spawn(
            normalize_usage(self.usage.as_ref()),
            self.content.encode_utf16().count(),
            &spend,
        );
    }
}

impl Stream for MeteredStream {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // SAFETY: no field of Self is structurally pinned — the only
        // address-sensitive data lives in `inner`, a Pin<Box> that pins its
        // own pointee, and no other field's address is ever handed out.
        let this = unsafe { self.get_unchecked_mut() };
        loop {
            if let Some(chunk) = this.out.pop_front() {
                return Poll::Ready(Some(Ok(chunk)));
            }
            if this.ended && this.guard.is_none() {
                return Poll::Ready(None);
            }
            // Annotate-mode clean end: await the guard, then the caveat (if
            // any) and the withheld [DONE] go out as the stream's last bytes.
            if let Some(g) = this.guard.as_mut() {
                if let Poll::Ready((_, caveat, _)) = g.as_mut().poll(cx) {
                    this.guard = None;
                    if !caveat.is_empty() {
                        this.out
                            .push_back(guard_caveat_chunk(&caveat, &this.guard_spec.model));
                    }
                    if this.saw_done {
                        this.out.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                    }
                    continue;
                }
                return Poll::Pending;
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.on_chunk(&chunk);
                    if this.mode == MeterMode::Passthrough {
                        // Passthrough: the client's bytes, untouched.
                        return Poll::Ready(Some(Ok(chunk)));
                    }
                    // Annotate: re-emitted lines come off the out queue.
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    // An error frame runs neither flush nor cancel — the
                    // spend settles, the guard does not run.
                    this.errored = true;
                    this.settle();
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => {
                    // Clean end: scan the tail, settle, run the guard.
                    this.ended = true;
                    this.flush_tail();
                    this.settle();
                    match this.mode {
                        MeterMode::Passthrough => {
                            this.guard_spec.observe(&this.content);
                        }
                        MeterMode::Annotate if this.content.is_empty() => {
                            if this.saw_done {
                                this.out.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
                            }
                        }
                        MeterMode::Annotate => {
                            this.guard = Some(this.guard_spec.call(&this.content));
                        }
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl Drop for MeteredStream {
    fn drop(&mut self) {
        self.settle();
        // The cancel path: the client walked away mid-stream. The spend
        // settles above; in annotate mode the guard still runs observe-posture
        // on what assembled — the caveat can't reach a stream nobody is
        // reading, but the findings still record. A clean end already ran the
        // guard; an upstream error frame runs nothing.
        if self.mode == MeterMode::Annotate && !self.ended && !self.errored {
            self.guard_spec.observe(&self.content);
        }
    }
}

// The scanner is pure line-processing — testable without a socket.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subject_is_the_agent_for_agent_keys_and_the_key_for_everyone_else() {
        // An operator's key, the two fleet-wide keys: `api:<name>`, as the
        // ledger has always spelled them.
        assert_eq!(caller_of("my-key", None), "api:my-key");
        assert_eq!(caller_of("fleet-gateway", None), "api:fleet-gateway");
        assert_eq!(
            caller_of("workbench-gateway", Some("ignored")),
            "api:workbench-gateway",
            "a scope never reaches a non-agent key"
        );
        // An agent's key: the model id the render scoped it with — the name
        // chat-persist ledgers the persona turn under — else its slug.
        assert_eq!(caller_of("agent:xeng", Some("x-eng")), "x-eng");
        assert_eq!(caller_of("agent:xeng", None), "xeng");
        // An app's key: the app, by its own name — the subject its budget
        // binds and the namespace its stamps book under.
        assert_eq!(caller_of("app:crm", None), "app:crm");
        assert_eq!(
            caller_of("app:Not A Slug", None),
            "api:app:Not A Slug",
            "only an app's shape is an app"
        );
    }

    #[test]
    fn every_call_is_metered_here_and_an_agents_starts_unattended() {
        // An operator's key: metered here, as ever — unless listed unmetered.
        assert_eq!(meter_plan(false, false, false), Meter::Book(RELAY_SOURCE));
        assert_eq!(meter_plan(false, false, true), Meter::Skip);
        // An agent's key, inside a Talaria turn or not: booked here, as the
        // agent's, until its conversation key names the flow.
        assert_eq!(
            meter_plan(true, false, false),
            Meter::Book(UNATTENDED_SOURCE)
        );
        // An operator listing an agent key as unmetered still wins.
        assert_eq!(meter_plan(true, false, true), Meter::Skip);
        // An app's key: the app's spend, under its own source.
        assert_eq!(meter_plan(false, true, false), Meter::Book(APP_SOURCE));
        assert_eq!(Meter::Book(APP_SOURCE).source(), "app");
        assert_eq!(meter_plan(false, true, true), Meter::Skip);
        // The ledger's view of a decision.
        assert!(Meter::Skip.skip());
        assert!(!Meter::Book(UNATTENDED_SOURCE).skip());
        assert_eq!(Meter::Book(UNATTENDED_SOURCE).source(), "unattended");
    }

    #[test]
    fn the_session_header_books_an_agents_calls_and_a_workbench_jobs_evals_only() {
        // An agent's own key: the conversation key Hermes stamped.
        assert!(honors_affinity(true, "agent:xeng"));
        // The workbench key: a job's eval session, booked to its ticket.
        assert!(honors_affinity(false, "workbench-gateway"));
        // prepare_env records a job's eval session under this caller
        // (talaria-workbench-mcp `WORKBENCH_CALLER`); the relay's insert
        // finds the flow only if the two agree.
        assert_eq!(caller_of(WORKBENCH_KEY, None), "api:workbench-gateway");
        // The personas' key and an operator's: their rows stay theirs.
        assert!(!honors_affinity(false, "fleet-gateway"));
        assert!(!honors_affinity(false, "my-key"));
    }

    fn scanner_mode(mode: MeterMode) -> MeteredStream {
        // skip:true — settle runs its scan logic, the ledger write doesn't
        // (no DB in unit tests; the write itself is exercised live).
        let ledger = Ledger {
            pg: sqlx::PgPool::connect_lazy("postgres://x").unwrap(),
            caller: "api:test".into(),
            endpoint: "e".into(),
            endpoint_class: "cloud".into(),
            provider: "openrouter".into(),
            upstream_model: "m".into(),
            prompt_chars: 0,
            skip: true,
            source: RELAY_SOURCE,
            affinity: None,
            stated: StatedFlow::default(),
        };
        let guard_spec = GuardSpec {
            pg: sqlx::PgPool::connect_lazy("postgres://x").unwrap(),
            caller: "api:test".into(),
            model: "test-model".into(),
            endpoint: "e".into(),
            messages: json!([]),
            enabled: true,
        };
        MeteredStream::new(futures_util::stream::empty(), ledger, mode, guard_spec)
    }

    fn scanner() -> MeteredStream {
        scanner_mode(MeterMode::Passthrough)
    }

    // #[tokio::test]: constructing the ledger's lazy PgPool arms sqlx's pool
    // reaper, which needs a runtime — the scan logic itself is pure.
    #[tokio::test]
    async fn sse_scan_assembles_content_and_usage_across_chunk_splits() {
        let mut s = scanner();
        // A usage frame, then a content delta whose `é` (0xC3 0xA9) is split
        // MID-CHARACTER across the two chunks, then [DONE]. Byte-buffering
        // (not lossy-decoding per chunk) is what reassembles it.
        s.on_chunk(&Bytes::from_static(
            b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"h\xc3",
        ));
        s.on_chunk(&Bytes::from_static(b"\xa9llo\"}}]}\n\ndata: [DONE]\n\n"));
        assert_eq!(s.content, "héllo");
        assert_eq!(
            s.usage.as_ref().unwrap()["prompt_tokens"],
            serde_json::json!(10)
        );
        // settle books only — it must be idempotent and scan nothing new.
        s.settle();
        s.settle();
        assert!(s.settled);
    }

    #[tokio::test]
    async fn partial_and_non_json_lines_are_ignored() {
        let mut s = scanner();
        s.on_chunk(&Bytes::from_static(
            b": keep-alive comment\ndata: not-json{",
        ));
        assert_eq!(s.content, "");
        assert!(s.usage.is_none());
        // The tail stays pending — settle books only; the CLEAN end scans it.
        assert!(!s.buf.is_empty());
        s.settle();
        assert!(!s.buf.is_empty()); // still pending — a cancel would drop it
        s.flush_tail();
        assert!(s.buf.is_empty()); // flushed (and still ignored — not JSON)
    }

    #[tokio::test]
    async fn annotate_relays_lines_with_done_withheld_and_tail_verbatim() {
        let mut s = scanner_mode(MeterMode::Annotate);
        s.on_chunk(&Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
        ));
        // The delta line re-emits byte-for-byte; the blank separator line too;
        // [DONE] is withheld; the blank AFTER [DONE] still goes out (it is a
        // line like any other — only [DONE] itself is withheld).
        assert_eq!(
            s.out.pop_front().unwrap(),
            Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n")
        );
        assert_eq!(s.out.pop_front().unwrap(), Bytes::from_static(b"\n"));
        assert_eq!(s.out.pop_front().unwrap(), Bytes::from_static(b"\n"));
        assert!(s.out.is_empty());
        assert!(s.saw_done);
        assert_eq!(s.content, "hi");
        // A partial tail at the clean end goes out verbatim, no newline added.
        s.on_chunk(&Bytes::from_static(b"data: {\"choices\""));
        s.flush_tail();
        assert_eq!(
            s.out.pop_front().unwrap(),
            Bytes::from_static(b"data: {\"choices\"")
        );
        // A [DONE] tail is withheld like a [DONE] line.
        s.buf = b"data: [DONE]".to_vec();
        s.flush_tail();
        assert!(s.out.is_empty());
        assert!(s.saw_done);
    }

    #[test]
    fn done_detection_and_the_caveat_chunk_shape() {
        assert!(line_is_done(b"data: [DONE]"));
        assert!(line_is_done(b"data:[DONE]"));
        assert!(line_is_done(b"data: [DONE]\r")); // \r belongs to the line
        assert!(!line_is_done(b"data: {\"x\":1}"));
        assert!(!line_is_done(b": [DONE]"));
        // The caveat chunk is byte-exact — the pinned bytes are the contract.
        let chunk = guard_caveat_chunk("\n\n--- caveat", "claude-3");
        assert_eq!(
            chunk,
            Bytes::from_static(
                b"data: {\"id\":\"talaria-guard\",\"object\":\"chat.completion.chunk\",\"model\":\"claude-3\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\\n\\n--- caveat\"},\"finish_reason\":null}]}\n\n"
            )
        );
    }

    #[test]
    fn prompt_chars_counts_utf16_and_json_serializations() {
        let msgs = serde_json::json!([
            {"role": "user", "content": "héllo"},          // 5 UTF-16 units
            {"role": "user", "content": {"text": "x"}},    // JSON: {"text":"x"} = 12
            {"role": "user"},                              // absent → "" → 2
            {"role": "user", "content": null},             // null → "" → 2
        ]);
        assert_eq!(prompt_chars_of(&msgs), 21);
    }
}

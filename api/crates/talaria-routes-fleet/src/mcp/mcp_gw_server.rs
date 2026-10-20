// /api/mcp/gw/{server}.
// The MCP gateway — the registry's ENFORCEMENT point. Agents never see an
// upstream URL or credential: their configs point here, the agent's own
// credential identifies the caller (agent-auth), and the gateway
//   · forwards JSON-RPC to the upstream (org headers, or the acting user's
//     connected-account headers on per-user servers)
//   · FILTERS tools/list down to the allowed set
//   · REJECTS tools/call outside it
//   · ANSWERS a brace-led body that isn't valid JSON with JSON-RPC -32700,
//     so a malformed tools/call cannot ride the raw pass-through around
//     the two gates above
//   · ANSWERS IN-HOUSE for a granted server nobody connected yet: a real
//     handshake, the cached catalog, and -32001 with the one sentence that
//     says who connects it and where (registry `McpAccess::NotConnected`,
//     shapes in mcp::not_connected) — distinct from the 403 a refusal gets
//   · REFUSES a JSON-RPC batch carrying tools/call or tools/list (-32600 per
//     member): the tool gate, the spend boundary and the list filter read a
//     single-object body, so a batch would ride past all three
//   · RECORDS every tools/call it answers — and every refusal, a batch's
//     members included — as one row in mcp_tool_calls (who, which server and
//     tool, outcome, latency, the error sentence scrubbed of the arguments;
//     never arguments or results), off the hot path (mcp::calls)
// so a hand-edited agent config can never exceed what the registry granted.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::{AgentSubject, presented, require_agent, subject_model};
use talaria_api_facades::mcp::calls::{
    ACCESS_FAILED, BATCH_REFUSED, CallLog, Outcome, TappedStream, batch_needs_refusal,
    classify_answer, recorded_members, upstream_text,
};
use talaria_api_facades::mcp::jsonrpc::rpc_error;
use talaria_api_facades::mcp::not_connected::{NotConnected, browser_base, house_answer};
use talaria_api_facades::mcp::registry::{McpAccess, effective_mcp_for, parse_mcp_response};
use talaria_error::{house_error, internal, upstream_error_message};
use talaria_session::secretbox_or_500;
use talaria_state::AppState;
use talaria_workspace_secrets::spend_handles_in_tool_call;

/// the called tool's name — the gate's key ("undefined" never matches a
/// tool list) and the rejection sentence's subject ("undefined" when
/// absent, the bare string otherwise).
fn called_tool(rpc: Option<&Value>) -> String {
    match rpc.and_then(|r| r.pointer("/params/name")) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(), // a non-string name embeds its serialized form
        None => "undefined".to_string(),
    }
}

fn gate(rpc: Option<&Value>, tools: Option<&Vec<String>>) -> Option<Response> {
    let tools = tools?;
    if rpc?.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    let called = called_tool(rpc);
    if tools.contains(&called) {
        return None;
    }
    let id = rpc
        .and_then(|r| r.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    Some(
        (
            StatusCode::OK,
            Json(rpc_error(
                &id,
                -32602,
                &format!("tool \"{called}\" is not available here"),
            )),
        )
            .into_response(),
    )
}

/// The in-house answer for a granted server whose account is not connected,
/// framed the way `gate` frames its refusal — a JSON-RPC body over HTTP 200,
/// which every MCP client hands to the model as the call's error — and a 202
/// with no body for a notification. Pure over its inputs so the framing is
/// pinned by tests without a running stack.
fn not_connected_reply(nc: &NotConnected, rpc: Option<&Value>, base: &str) -> Response {
    match house_answer(nc, rpc, base) {
        (status, None) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::ACCEPTED),
            Body::empty(),
        )
            .into_response(),
        (status, Some(out)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
            Json(out),
        )
            .into_response(),
    }
}

/// A body that starts (ignoring whitespace) with an opening brace is a
/// JSON-RPC attempt; one that failed to parse is a syntax error in that
/// attempt, and forwarding it verbatim would carry a malformed tools/call
/// straight past the tool gate and the spend boundary below. Answer it
/// here instead, as a JSON-RPC parse error over HTTP 200 — the same shape
/// the tool gate answers with — and log a bounded diagnostic. Every other
/// unparseable shape (an empty body, SSE framing, a `[`-led batch, a
/// ping) keeps the pass-through untouched, so `None` means exactly
/// "forward as before". `server_name` only names the log line; tests
/// drive the rest with the raw bytes and serde's own error.
fn parse_error_response(
    body: &[u8],
    err: &serde_json::Error,
    server_name: &str,
) -> Option<Response> {
    if !body.trim_ascii().starts_with(b"{".as_slice()) {
        return None;
    }
    let len = body.len();
    let diagnostic = parse_diagnostic(body, err);
    tracing::warn!("[mcp/gw:{server_name}] unparseable body ({len} bytes): {err} | {diagnostic}");
    Some(
        (
            StatusCode::OK,
            Json(rpc_error(
                &Value::Null,
                -32700,
                &format!(
                    "request body is not valid JSON: {err}. The call was refused here, not \
                     forwarded: the hexdump below is what the caller sent, so the fix is on the \
                     sender's serialization (nested tool arguments need one \\ escape per JSON \
                     layer). {diagnostic}"
                ),
            )),
        )
            .into_response(),
    )
}

/// The bounded diagnostic for a JSON parse failure: the byte offset of the
/// failure and a hexdump of the 8 bytes before it through the 8 after —
/// hex pairs plus a printable-ASCII rendering, a dot for anything else.
/// Those windowed bytes are the ONLY body content that ever reaches a log
/// line or an error message, because bodies carry tool arguments and
/// credentials; the offset plus at most 16 hex pairs and 16 ASCII chars
/// keeps the diagnostic itself under 200 chars, and the full message under
/// 450 even with serde's wording and the caller-side hint.
fn parse_diagnostic(body: &[u8], err: &serde_json::Error) -> String {
    let offset = error_byte_offset(body, err);
    let window = &body[offset.saturating_sub(8)..offset.saturating_add(8).min(body.len())];
    let hex = window
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let ascii: String = window
        .iter()
        .map(|&b| {
            if (0x20..=0x7e).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    format!("offset {offset} | {hex} | {ascii}")
}

/// The byte offset a serde error points at. serde_json reports a 1-based
/// line and a 0-based column ("column" is bytes since the line's last
/// newline), so walking to the reported line's start recovers the offset.
/// A body too short for the reported position clamps to its end.
fn error_byte_offset(body: &[u8], err: &serde_json::Error) -> usize {
    let mut line_start = 0;
    for _ in 1..err.line() {
        match body[line_start..].iter().position(|&b| b == b'\n') {
            Some(at) => line_start += at + 1,
            None => return body.len(),
        }
    }
    (line_start + err.column()).min(body.len())
}

/// Write the call's row when this request is a recorded tools/call. Every
/// branch that ANSWERS one writes through this; the streamed relay writes its
/// own when the stream ends.
fn note(call: &Option<CallLog>, outcome: Outcome, status: u16, error: Option<&str>) {
    if let Some(c) = call {
        c.finish(outcome, Some(status), error);
    }
}

/// Did the spend boundary put a credential into this call?
fn spent(call: &Option<CallLog>) -> bool {
    call.as_ref().is_some_and(CallLog::spent)
}

/// The refusal for a batch the gateway will not relay (see
/// `batch_needs_refusal`): a JSON-RPC batch answer over HTTP 200 — one
/// -32600 per member that carries an id, none for a notification — or a
/// single -32600 with a null id when no member could take a reply. None for
/// anything else, which relays as before. Pure, so the shape is pinned
/// without a running stack.
fn batch_refusal(rpc: Option<&Value>) -> Option<Response> {
    if !batch_needs_refusal(rpc) {
        return None;
    }
    let replies: Vec<Value> = rpc
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id"))
        .map(|id| rpc_error(id, -32600, BATCH_REFUSED))
        .collect();
    let body = if replies.is_empty() {
        rpc_error(&Value::Null, -32600, BATCH_REFUSED)
    } else {
        Value::Array(replies)
    };
    Some((StatusCode::OK, Json(body)).into_response())
}

pub async fn post(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // The call record's clock: the agent's whole wait at the gateway.
    let started = std::time::Instant::now();
    let caller = require_agent(&state.pg, &headers).await?;
    // Pass the CALLER, never `caller.model`. `subject_model`/`subject_proven`
    // read a bare string as PROVEN, so downgrading to the name here throws
    // away `legacy` — and this route is where that matters most: it resolves
    // the acting owner and can put that human's OAuth bearer token into
    // `upstreamHeaders`. `name` below is only ever used where a header or an
    // unthreaded callee genuinely needs the string.
    let subject = AgentSubject::Caller(caller.clone());
    let name = caller.model.clone();
    // Parsed ONCE: the record, the in-house answer and the gate below all
    // read this. A failure is kept for the parse-error answer further down.
    let parsed = serde_json::from_slice::<Value>(&body);
    // One log per tools/call the body carries — the body itself, or each
    // tools/call member of a batch.
    let members = recorded_members(parsed.as_ref().ok());
    let logs: Vec<CallLog> = members
        .iter()
        .filter_map(|m| CallLog::begin(&state.pg, &subject, &server_name, Some(m), started))
        .collect();
    // A failure inside the gateway before access is known is still a call
    // the agent made and did not get answered: an outage shows up in the
    // record as a burst of these, not as silence.
    let fail_all = |logs: &[CallLog]| {
        for c in logs {
            c.finish(Outcome::Error, Some(500), Some(ACCESS_FAILED));
        }
    };
    let sb = match secretbox_or_500(&state, "[mcp/gw] secretbox unavailable").await {
        Ok(sb) => sb,
        Err(resp) => {
            fail_all(&logs);
            return Err(resp);
        }
    };
    let access = match effective_mcp_for(&state.pg, &sb, &subject, &server_name).await {
        Ok(a) => a,
        Err(e) => {
            fail_all(&logs);
            return Ok(internal("[mcp/gw] effective resolution failed", e));
        }
    };
    let mut eff = match access {
        McpAccess::Allowed(eff) => eff,
        McpAccess::Denied => {
            const NO_ACCESS: &str = "no access to this MCP server";
            for c in &logs {
                c.finish(Outcome::Denied, Some(403), Some(NO_ACCESS));
            }
            return Ok(house_error(StatusCode::FORBIDDEN, NO_ACCESS));
        }
        // Granted, but nobody connected the account it would speak as.
        // Nothing is forwarded (so nothing is spent): the handshake and the
        // catalog are answered here and every call carries the sentence that
        // names the fix. The link is the instance's own address — the
        // verified domain when there is one, else AUTH_PUBLIC_URL, else the
        // origin this request arrived on (the OAuth callback's base) — as
        // the PERSON's browser opens it: this request came from an agent
        // container, whose origin for a host-run app is host.docker.internal.
        McpAccess::NotConnected(nc) => {
            let rpc = parsed.as_ref().ok();
            let base = browser_base(&super::oauth_callback_base(&state, &headers, &uri).await);
            let method = rpc
                .and_then(|r| r.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if method == "tools/call" {
                tracing::info!(
                    "[mcp/gw:{server_name}] {name} called {} on a server that is not connected ({:?})",
                    called_tool(rpc),
                    nc.scope
                );
            }
            // Each call's row carries the sentence its own answer handed
            // the agent — a batch's members are answered one by one in-house.
            for (member, c) in members.iter().zip(logs) {
                let (status, out) = house_answer(&nc, Some(member), &base);
                let sentence = out
                    .as_ref()
                    .and_then(|o| o.pointer("/error/message"))
                    .and_then(Value::as_str);
                c.with_server(&nc.server.id)
                    .finish(Outcome::NotConnected, Some(status), sentence);
            }
            return Ok(not_connected_reply(&nc, rpc, &base));
        }
    };
    let logs: Vec<CallLog> = logs
        .into_iter()
        .map(|c| c.with_server(&eff.server.id))
        .collect();

    // A batch that carries a tools/call or a tools/list would ride past the
    // tool gate, the spend boundary and the catalog filter, which all read a
    // single-object body — so it is refused here, before anything is
    // forwarded, and each tools/call inside it is recorded as refused. (The
    // 2025-06 MCP revision dropped batching; no current client sends one.)
    if let Some(resp) = batch_refusal(parsed.as_ref().ok()) {
        tracing::warn!(
            "[mcp/gw:{server_name}] {name} sent a batch carrying tools/call or tools/list; refused"
        );
        for c in &logs {
            c.finish(Outcome::Denied, Some(200), Some(BATCH_REFUSED));
        }
        return Ok(resp);
    }
    // From here the body is at most one tools/call.
    let mut call: Option<CallLog> = logs.into_iter().next();

    // A body that parses runs the gate and the spend boundary below. One
    // that doesn't still passes through verbatim — EXCEPT a brace-led body,
    // a JSON-RPC attempt with a syntax error in it, which must not skip
    // them: it is answered as a JSON-RPC parse error instead (above).
    let mut rpc: Option<Value> = match parsed {
        Ok(v) => Some(v),
        Err(err) => match parse_error_response(&body, &err, &server_name) {
            Some(resp) => return Ok(resp),
            None => None,
        },
    };
    let mut body_text = String::from_utf8_lossy(&body).into_owned();

    // The call gate: reject disallowed tools before the upstream ever
    // hears about them.
    if let Some(resp) = gate(rpc.as_ref(), eff.tools.as_ref()) {
        note(
            &call,
            Outcome::Denied,
            200,
            Some(&format!(
                "tool \"{}\" is not available here",
                called_tool(rpc.as_ref())
            )),
        );
        return Ok(resp);
    }

    // THE BOUNDARY THAT SPENDS A CREDENTIAL. An agent holds
    // `«secret:deploy.github_pat»` and passes it wherever the value would go;
    // this is where the value actually appears, on its way OUT. It has to be
    // here, before every dispatch below, because this route is the only thing an
    // agent's tool call goes through — see `spend_handles_in_tool_call` for
    // what forwarding the handle verbatim looked like.
    //
    // The in-process branch takes the mutated `rpc`; the HTTP one
    // re-serializes, and only when something was actually spent. An
    // unresolved handle is reported to the OPERATOR and never back to the
    // model: a caller that learns which names exist has been handed a map of
    // the workspace's credentials.
    if let Some(rpc_mut) = rpc.as_mut() {
        let spend = match spend_handles_in_tool_call(&state.pg, &sb, rpc_mut, &name).await {
            Ok(s) => s,
            Err(e) => {
                note(
                    &call,
                    Outcome::SpendError,
                    500,
                    Some("the credential spend boundary failed; the call was not forwarded"),
                );
                return Ok(internal("[mcp/gw] spend boundary failed", e));
            }
        };
        let tool = called_tool(Some(rpc_mut));
        for u in &spend.used {
            tracing::warn!(
                "[secrets] {name} spent {}.{} ({}) on {server_name}.{tool}",
                u.name,
                u.key,
                u.label
            );
        }
        for u in &spend.unresolved {
            tracing::warn!(
                "[secrets] {name} could not resolve {} on {server_name}.{tool}: {}",
                u.handle,
                u.reason
            );
        }
        if spend.changed {
            body_text = serde_json::to_string(rpc_mut).unwrap_or_else(|_| body_text.clone());
            // The arguments now hold a real credential, and an upstream that
            // rejects one may quote it back: no reply text is kept for it.
            if let Some(c) = call.as_mut() {
                c.mark_spent();
            }
        }
    }

    // The Workbench surface dispatches IN-PROCESS with the caller's agent
    // identity — grants resolved by the same gateway rules as any server.
    if eff.server.url.starts_with("talaria-workbench://") {
        let deps = talaria_api_facades::workbench::mcp::WorkbenchDeps {
            pg: state.pg.clone(),
            sb: sb.clone(),
            redis: state.redis().await.ok(),
        };
        let rpc_body = rpc.clone().unwrap_or_else(|| json!({}));
        let (status, out) = talaria_api_facades::workbench::mcp::dispatch_workbench_mcp(
            &deps,
            &rpc_body,
            &subject,
            eff.tools.as_deref(),
        )
        .await;
        let (outcome, error) = classify_answer(status.as_u16(), out.as_ref(), spent(&call));
        note(&call, outcome, status.as_u16(), error.as_deref());
        return Ok(match out {
            None => (status, Body::empty()).into_response(),
            Some(out) => (status, Json(out)).into_response(),
        });
    }

    // Package servers (npm/pypi/oci from the marketplace): stdio packages
    // converse through the pump this process owns — the tool gate above
    // already ran on the same rpc body; oci-http packages relay like
    // remotes against the container's resolved URL (a loopback or container
    // DNS address, so the SSRF check below is skipped for them the same way
    // it is for the builtin toolkit).
    if let Some(spec) = eff
        .server
        .package
        .as_ref()
        .and_then(talaria_api_facades::mcp::pkg::PkgSpec::of)
    {
        if spec.transport == "http" {
            let url =
                match talaria_api_facades::mcp::pkg::ensure_http(&sb, &eff.server, &spec).await {
                    Ok(u) => u,
                    Err(e) => {
                        note(&call, Outcome::UpstreamError, 502, Some(&e));
                        return Ok(house_error(StatusCode::BAD_GATEWAY, &e));
                    }
                };
            eff.server = talaria_api_facades::mcp::registry::McpServer {
                url,
                ..eff.server.clone()
            };
        } else {
            let rpc_body = rpc.clone().unwrap_or_else(|| json!({}));
            return Ok(
                match talaria_api_facades::mcp::pkg::pkg_call(
                    &state.pg,
                    &sb,
                    &eff.server,
                    &spec,
                    &rpc_body,
                )
                .await
                {
                    Ok((status, out)) if out.is_null() => {
                        let (outcome, error) = classify_answer(status, None, spent(&call));
                        note(&call, outcome, status, error.as_deref());
                        (
                            StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                            Body::empty(),
                        )
                            .into_response()
                    }
                    Ok((status, out)) => {
                        let (outcome, error) = classify_answer(status, Some(&out), spent(&call));
                        note(&call, outcome, status, error.as_deref());
                        (
                            StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                            Json(out),
                        )
                            .into_response()
                    }
                    Err(e) => {
                        // The pump's sentence can carry the child's own
                        // words about the arguments it was handed.
                        let kept = upstream_text(&e, spent(&call));
                        note(&call, Outcome::UpstreamError, 502, Some(&kept));
                        house_error(StatusCode::BAD_GATEWAY, &e)
                    }
                },
            );
        }
    }

    // App-published servers dispatch IN-PROCESS in TS through the app module
    // — authors' TS/node code, which rule 10 keeps on the TS side (the
    // never-port surface). `app-*` servers live there, not here; a direct
    // hit answers the boundary sentence instead of pretending to dispatch.
    if let Some(app_slug) = &eff.server.app_slug {
        let sentence = format!(
            "app \"{app_slug}\" dispatches in-process through the app runtime, which stays TS by rule 10 (docs/RUST-MIGRATION.md)"
        );
        note(&call, Outcome::Error, 502, Some(&sentence));
        return Ok(house_error(StatusCode::BAD_GATEWAY, &sentence));
    }

    // The builtin toolkit is a child of THIS process, spawned on demand —
    // and a deploy's first agent session can beat the spawn. A session
    // whose initialize fails drops the server (and with it every talaria
    // tool) for its whole lifetime, so the relay heals the race instead of
    // reporting it: bring the child up and wait, then let the fetch below
    // speak for the outcome either way. A live child answers the probe in
    // milliseconds; only a dead one pays the wait.
    if eff.server.builtin {
        talaria_api_facades::mcp::service::await_mcp_service(8_000).await;
    }

    // Upstream URLs are admin-entered, not first-party: validate the hop
    // through the same door every other registry URL walks (mcp-registry's
    // session path uses safeFetch). The relay itself stays a raw pass-through
    // fetch — a streamable-HTTP response can be an endless SSE stream, which
    // safeFetch's response cap would buffer and kill — so the URL is checked
    // BEFORE the hop and the response streamed as before. The BUILTIN toolkit
    // is this process's own loopback listener and skips the check (loopback is
    // exactly what it refuses).
    if !eff.server.builtin
        && eff.server.package.is_none()
        && talaria_safe_fetch::assert_fetchable_url(&eff.server.url)
            .await
            .is_err()
    {
        const REFUSED: &str = "upstream URL refused (not a reachable external address)";
        note(&call, Outcome::UpstreamError, 502, Some(REFUSED));
        return Ok(house_error(StatusCode::BAD_GATEWAY, REFUSED));
    }

    // Header assembly order: content-type, accept,
    // [mcp-session-id], ...upstreamHeaders, and for the builtin toolkit the
    // caller's OWN credential — the toolkit calls back into this API as the
    // same agent, so substituting a server-held key here would make that hop
    // the forgeable one.
    let hdr = |name: &'static str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let mut req = talaria_api_facades::gateway::provider::http()
        .post(&eff.server.url)
        .timeout(std::time::Duration::from_millis(
            (eff.server.timeout_secs.unwrap_or(120) as u64) * 1000,
        ));
    req = req.header(
        "content-type",
        hdr("content-type").unwrap_or_else(|| "application/json".into()),
    );
    req = req.header(
        "accept",
        hdr("accept").unwrap_or_else(|| "application/json, text/event-stream".into()),
    );
    if let Some(session) = hdr("mcp-session-id") {
        req = req.header("mcp-session-id", session);
    }
    for (k, v) in &eff.upstream_headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if eff.server.builtin {
        req = req.header("X-Agent-Name", &name);
        req = req.header("X-Api-Key", presented(&headers).unwrap_or_default());
    }

    let upstream = match req.body(body_text).send().await {
        Ok(r) => r,
        Err(e) => {
            // The fetch error can name the upstream host (ECONNREFUSED
            // host:port) — agent configs are hand-editable; endpoint
            // topology isn't theirs.
            talaria_error::log_upstream_error(
                &format!("mcp-gw:{server_name}"),
                "unreachable",
                &e.to_string(),
            );
            note(
                &call,
                Outcome::UpstreamError,
                502,
                Some("upstream unreachable"),
            );
            return Ok(house_error(StatusCode::BAD_GATEWAY, "upstream unreachable"));
        }
    };
    Ok(relay(
        upstream,
        rpc.as_ref(),
        eff.tools.as_deref(),
        &server_name,
        call,
    )
    .await)
}

/// GET — the streamable-HTTP notification stream (server → client): plain
/// relay, same gates as POST.
pub async fn get(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    // Same rule as POST: the caller carries the proof, the string does not.
    let subject = AgentSubject::Caller(caller);
    let sb = secretbox_or_500(&state, "[mcp/gw] secretbox unavailable").await?;
    let access = match effective_mcp_for(&state.pg, &sb, &subject, &server_name).await {
        Ok(a) => a,
        Err(e) => return Ok(internal("[mcp/gw] effective resolution failed", e)),
    };
    let mut eff = match access {
        McpAccess::Allowed(eff) => eff,
        McpAccess::Denied => {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "no access to this MCP server",
            ));
        }
        // A not-connected server has no upstream session to stream from; 405
        // is the streamable-HTTP "no notification stream", which clients take
        // in stride (the POST side carries the sentence).
        McpAccess::NotConnected(_) => {
            return Ok((StatusCode::METHOD_NOT_ALLOWED, Body::empty()).into_response());
        }
    };
    // App servers have no notification stream — decline politely.
    if eff.server.app_slug.is_some() {
        return Ok((StatusCode::METHOD_NOT_ALLOWED, Body::empty()).into_response());
    }
    // A stdio package has no notification stream either; an oci-http one
    // relays to its container's resolved URL.
    if let Some(spec) = eff
        .server
        .package
        .as_ref()
        .and_then(talaria_api_facades::mcp::pkg::PkgSpec::of)
    {
        if spec.transport == "http" {
            let url =
                match talaria_api_facades::mcp::pkg::ensure_http(&sb, &eff.server, &spec).await {
                    Ok(u) => u,
                    Err(e) => return Ok(house_error(StatusCode::BAD_GATEWAY, &e)),
                };
            eff.server = talaria_api_facades::mcp::registry::McpServer {
                url,
                ..eff.server.clone()
            };
        } else {
            return Ok((StatusCode::METHOD_NOT_ALLOWED, Body::empty()).into_response());
        }
    }
    // Same heal as POST: the builtin child may not be up when the client
    // opens its notification stream.
    if eff.server.builtin {
        talaria_api_facades::mcp::service::await_mcp_service(8_000).await;
    }

    // Same rule as POST: validate a non-builtin upstream URL before the hop;
    // the response is a live SSE relay, so the fetch itself stays raw.
    if !eff.server.builtin
        && eff.server.package.is_none()
        && talaria_safe_fetch::assert_fetchable_url(&eff.server.url)
            .await
            .is_err()
    {
        return Ok(house_error(
            StatusCode::BAD_GATEWAY,
            "upstream URL refused (not a reachable external address)",
        ));
    }
    let hdr = |name: &'static str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let mut req = talaria_api_facades::gateway::provider::http().get(&eff.server.url);
    req = req.header(
        "accept",
        hdr("accept").unwrap_or_else(|| "text/event-stream".into()),
    );
    if let Some(session) = hdr("mcp-session-id") {
        req = req.header("mcp-session-id", session);
    }
    for (k, v) in &eff.upstream_headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if eff.server.builtin {
        req = req.header("X-Agent-Name", subject_model(&subject));
        req = req.header("X-Api-Key", presented(&headers).unwrap_or_default());
    }
    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            talaria_error::log_upstream_error(
                &format!("mcp-gw-get:{server_name}"),
                "unreachable",
                &e.to_string(),
            );
            return Ok(house_error(StatusCode::BAD_GATEWAY, "upstream unreachable"));
        }
    };
    // A failed notification-stream hop relays the same way as POST: fixed
    // sentence, verbatim to the log. Only a live 200 SSE stream passes.
    let status = upstream.status().as_u16();
    if !(200..300).contains(&status) {
        let text = upstream.text().await.unwrap_or_default();
        talaria_error::log_upstream_error(&format!("mcp-gw-get:{server_name}"), status, &text);
        return Ok(house_error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            &upstream_error_message(status),
        ));
    }
    let ct = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    Ok(Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|e| internal("[fleet] response build failed", e)))
}

/// `call` is the tools/call's record, when this is one: a failed hop writes it
/// here, and a 2xx answer carries it inside the streamed body, which writes it
/// when the stream ends — the relay itself still never buffers.
async fn relay(
    upstream: reqwest::Response,
    rpc: Option<&Value>,
    tools: Option<&[String]>,
    server_name: &str,
    call: Option<CallLog>,
) -> Response {
    let status = upstream.status().as_u16();
    // HTTP-level failures never relay verbatim: their bodies are written by
    // whatever proxy or server answered, not by the MCP protocol. One check
    // here covers every relay below (tools/list filter, its fallback, the
    // final stream). JSON-RPC errors ride 200s and pass untouched — tool
    // results, including tool FAILURES, are the protocol the agent speaks.
    if !(200..300).contains(&status) {
        let text = upstream.text().await.unwrap_or_default();
        talaria_error::log_upstream_error(&format!("mcp-gw:{server_name}"), status, &text);
        let sentence = upstream_error_message(status);
        if let Some(c) = &call {
            c.finish(Outcome::UpstreamError, Some(status), Some(&sentence));
        }
        return house_error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            &sentence,
        );
    }
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let session = upstream
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let mut builder = Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK))
        .header(header::CONTENT_TYPE, &content_type);
    if let Some(session) = &session {
        builder = builder.header("mcp-session-id", session);
    }

    // The list filter: rewrite result.tools inside JSON or SSE-framed
    // bodies. Everything else streams back verbatim.
    if rpc
        .map(|r| r.get("method"))
        .and_then(|m| m.and_then(Value::as_str))
        == Some("tools/list")
        && let Some(allowed) = tools
    {
        let text = upstream.text().await.unwrap_or_default();
        let out = filter_bodies(&text, allowed, &content_type);
        return builder
            .body(Body::from(out))
            .unwrap_or_else(|e| internal("[fleet] response build failed", e));
    }
    let body = match call {
        Some(c) => Body::from_stream(TappedStream::new(
            upstream.bytes_stream(),
            c,
            status,
            &content_type,
        )),
        None => Body::from_stream(upstream.bytes_stream()),
    };
    builder
        .body(body)
        .unwrap_or_else(|e| internal("[fleet] response build failed", e))
}

/// The tools/list filter over JSON or SSE-framed text: `data:` lines are
/// parsed, filtered, and re-serialized in place (`data: ${JSON.stringify(...)}` with
/// the single-space separator); anything unparseable passes through
/// untouched, and a non-SSE body is parsed whole — or passed verbatim when
/// it isn't parseable either.
fn filter_bodies(text: &str, allowed: &[String], content_type: &str) -> String {
    fn filter_msg(msg: &mut Value, allowed: &[String]) {
        if let Some(tools) = msg
            .pointer_mut("/result/tools")
            .and_then(Value::as_array_mut)
        {
            tools.retain(|t| {
                t.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| allowed.iter().any(|a| a == n))
            });
        }
    }
    if content_type.contains("text/event-stream") {
        return text
            .split('\n')
            .map(|line| {
                let rest = line.strip_prefix("data:");
                match rest.and_then(|r| serde_json::from_str::<Value>(r.trim()).ok()) {
                    Some(mut msg) => {
                        filter_msg(&mut msg, allowed);
                        format!("data: {msg}")
                    }
                    None => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    match parse_mcp_response(text) {
        Some(mut msg) => {
            filter_msg(&mut msg, allowed);
            serde_json::to_string(&msg).unwrap_or_else(|_| text.to_string())
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diagnostic serde's own error produces for these bytes — the
    /// window, the offset recovery, and the hex/ASCII rendering.
    fn diagnostic(body: &[u8]) -> String {
        let err = serde_json::from_slice::<Value>(body).expect_err("test bodies do not parse");
        parse_diagnostic(body, &err)
    }

    async fn body_of(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("test responses have a body");
        serde_json::from_slice(&bytes).expect("test responses are JSON")
    }

    async fn jsonrpc_of(resp: Response) -> Value {
        let v = body_of(resp).await;
        assert_eq!(
            v.get("jsonrpc"),
            Some(&json!("2.0")),
            "the answer is a JSON-RPC 2.0 envelope"
        );
        v
    }

    fn not_connected(scope: talaria_api_facades::mcp::not_connected::ConnectScope) -> NotConnected {
        NotConnected {
            scope,
            server: talaria_api_facades::mcp::registry::McpServer {
                id: "00000000-0000-0000-0000-000000000004".into(),
                name: "notion".into(),
                label: "Notion".into(),
                description: None,
                url: "https://mcp.notion.com/mcp".into(),
                headers_plain: serde_json::Map::new(),
                headers_sealed: None,
                timeout_secs: None,
                enabled: true,
                all_agents: false,
                auth_mode: "per-user".into(),
                tools: json!([{ "name": "search", "description": "Search pages" }]),
                tools_refreshed_ms: None,
                required_headers: json!([]),
                oauth_enabled: true,
                builtin: false,
                app_slug: None,
                created_by: None,
                created_ms: 0,
                package: None,
                env_enc: None,
                last_probe: None,
            },
            tools: None,
        }
    }

    #[tokio::test]
    async fn a_not_connected_call_is_framed_like_the_gate_with_the_connect_sentence() {
        use talaria_api_facades::mcp::not_connected::{ConnectScope, NOT_CONNECTED_CODE};
        let base = "http://localhost:5273";
        for (scope, expected) in [
            (
                ConnectScope::PerUser,
                "Your Notion account isn't connected to Talaria. Connect it at \
                 http://localhost:5273/settings/connections, then try again.",
            ),
            (
                ConnectScope::Org,
                "The workspace's Notion connection isn't set up yet. An admin connects it on the \
                 MCP page at http://localhost:5273/mcp/external.",
            ),
            (
                ConnectScope::PerUserOnOrgAgent,
                "Notion is a per-user server; only a personal assistant can use it.",
            ),
        ] {
            let nc = not_connected(scope);
            let call = json!({ "jsonrpc": "2.0", "id": 11, "method": "tools/call", "params": { "name": "search" } });
            let resp = not_connected_reply(&nc, Some(&call), base);
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "the gate's framing: HTTP 200"
            );
            let v = jsonrpc_of(resp).await;
            assert_eq!(v.pointer("/id"), Some(&json!(11)));
            assert_eq!(v.pointer("/error/code"), Some(&json!(NOT_CONNECTED_CODE)));
            assert_eq!(
                v.pointer("/error/message").and_then(Value::as_str),
                Some(expected)
            );

            // The handshake succeeds, so the harness keeps the server.
            let init = json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {} });
            let resp = not_connected_reply(&nc, Some(&init), base);
            assert_eq!(resp.status(), StatusCode::OK);
            let v = jsonrpc_of(resp).await;
            assert!(v.get("error").is_none(), "initialize is a handshake: {v}");
            assert_eq!(v.pointer("/result/serverInfo/name"), Some(&json!("notion")));

            // tools/list answers the cached catalog.
            let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
            let v = jsonrpc_of(not_connected_reply(&nc, Some(&list), base)).await;
            assert_eq!(v.pointer("/result/tools/0/name"), Some(&json!("search")));
        }
        // The initialized notification is a bodiless 202, like the in-process
        // dispatchers answer it.
        let nc = not_connected(ConnectScope::PerUser);
        let resp = not_connected_reply(
            &nc,
            Some(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
            base,
        );
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(bytes.is_empty());

        // A batch is answered with an array over HTTP 200 — one reply per
        // request, none for the notification inside it.
        let batch = json!([
            { "jsonrpc": "2.0", "id": 1, "method": "tools/list" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "search" } },
        ]);
        let resp = not_connected_reply(&nc, Some(&batch), base);
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_of(resp).await;
        let replies = v.as_array().expect("a batch answers a JSON array");
        assert_eq!(replies.len(), 2);
        assert_eq!(
            replies[0].pointer("/result/tools/0/name"),
            Some(&json!("search"))
        );
        assert_eq!(
            replies[1].pointer("/error/code"),
            Some(&json!(NOT_CONNECTED_CODE))
        );
    }

    /// The connect link the agent repeats must open in the person's browser:
    /// the agent-side alias of a host-run app becomes localhost.
    #[tokio::test]
    async fn the_connect_link_never_names_the_agents_docker_alias() {
        use talaria_api_facades::mcp::not_connected::ConnectScope;
        let nc = not_connected(ConnectScope::PerUser);
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "search" } });
        let base = browser_base("http://host.docker.internal:5303");
        let v = jsonrpc_of(not_connected_reply(&nc, Some(&call), &base)).await;
        let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert!(
            msg.contains("http://localhost:5303/settings/connections"),
            "{msg}"
        );
        assert!(!msg.contains("host.docker.internal"), "{msg}");
    }

    #[tokio::test]
    async fn a_raw_control_byte_in_a_string_is_a_32700_with_the_0a_in_the_dump() {
        // A tools/call whose argument string contains a REAL 0x0a — serde
        // rejects raw control characters inside strings, so this is exactly
        // the malformed tools/call the verbatim pass-through used to forward
        // around the gate and the spend boundary.
        let mut body: Vec<u8> = br#"{"method":"tools/call","params":{"name":"x","arg":"a"#.to_vec();
        body.push(0x0a); // lands INSIDE the still-open "a" string
        body.extend_from_slice(b"b\"}}");
        let err = serde_json::from_slice::<Value>(&body).expect_err("the raw 0x0a is rejected");

        let resp = parse_error_response(&body, &err, "srv")
            .expect("a brace-led body that fails to parse is answered, not forwarded");
        let v = jsonrpc_of(resp).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
        assert_eq!(v.pointer("/id"), Some(&Value::Null));
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert!(
            message.contains("control character"),
            "serde's own wording carries: {message}"
        );
        assert!(
            message.contains(" 0a "),
            "the hexdump window must include the offending byte: {message}"
        );
        assert!(message.len() < 450, "the diagnostic is bounded: {message}");
    }

    #[tokio::test]
    async fn three_braces_report_the_offset_and_a_bounded_dump() {
        let body = b"{{{";
        let err = serde_json::from_slice::<Value>(body).expect_err("{{{ does not parse");
        let resp = parse_error_response(body, &err, "srv")
            .expect("a JSON-RPC attempt with a syntax error is answered");
        let v = jsonrpc_of(resp).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert!(
            message.contains("offset 2"),
            "the failing byte (the second open brace) is named: {message}"
        );
        assert!(message.len() < 450, "the diagnostic is bounded: {message}");
        assert_eq!(
            diagnostic(body),
            "offset 2 | 7b 7b 7b | {{{",
            "a body shorter than the window dumps whole"
        );
    }

    #[tokio::test]
    async fn an_empty_body_passes_through_unanswered() {
        // No brace → not a JSON-RPC attempt → the pass-through stands, so
        // the batch/ping posture is preserved untouched.
        let err = serde_json::from_slice::<Value>(&[]).expect_err("an empty body is not JSON");
        assert!(
            parse_error_response(&[], &err, "srv").is_none(),
            "an empty body must not produce a -32700; it forwards as before"
        );
    }

    #[tokio::test]
    async fn a_valid_tools_call_body_still_reaches_the_gate_path() {
        // No AppState needed: `gate` is a pure function over the parsed rpc
        // and the allowed tools, so this pins the parse→gate contract the
        // change must keep intact — the body parses, and a disallowed name
        // is refused with the gate's own sentence.
        let body = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"rm-rf"}}"#;
        let rpc = serde_json::from_slice::<Value>(&body[..])
            .expect("a well-formed tools/call still parses");
        let tools = Some(vec!["ls".to_string()]);
        let resp =
            gate(Some(&rpc), tools.as_ref()).expect("the disallowed tool is refused by the gate");
        let v = body_of(resp).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32602)));
        assert_eq!(v.pointer("/id"), Some(&json!(7)));
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert_eq!(message, "tool \"rm-rf\" is not available here");
    }

    #[tokio::test]
    async fn a_markdown_ticket_body_with_raw_newlines_is_32700_with_the_caller_hint() {
        // The Sep 24 create_ticket signature: a markdown-heavy body whose
        // nested JSON was emitted with one escape layer too few, so REAL
        // newlines sit inside the argument string. The answer must refuse
        // here (never forward), carry serde's control-character wording,
        // and name the sender's serialization as the thing to fix — what
        // the bare parse error never said.
        let prefix = "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"create_ticket\",\"arguments\":{\"description\":\"## What's broken - the gateway rejected a valid filing."
            .as_bytes()
            .to_vec();
        let mut body = prefix.clone();
        body.push(0x0a);
        body.push(0x0a);
        body.extend_from_slice(b"## Evidence and fix\"}}");

        let err = serde_json::from_slice::<Value>(&body)
            .expect_err("raw newlines inside a string are refused");
        assert!(
            err.to_string().contains("control character"),
            "the failure is the control character: {err}"
        );
        let resp = parse_error_response(&body, &err, "srv")
            .expect("a brace-led JSON-RPC attempt is answered, not forwarded");
        let v = jsonrpc_of(resp).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert!(
            message.contains("the fix is on the sender"),
            "the caller-side hint must name which side to fix: {message}"
        );
        assert!(
            message.contains(" 0a "),
            "the hexdump window shows the raw newline bytes: {message}"
        );
        assert!(
            message.len() < 450,
            "the full answer stays bounded: {message}"
        );
    }

    #[tokio::test]
    async fn a_unicode_payload_never_leaks_its_text_into_the_answer() {
        // Tool arguments carry prose in every script. Multibyte characters
        // are legal JSON — the raw control character is the defect — and the
        // diagnostic window renders them as hex pairs and dots, so the
        // answer names the failing BYTE without echoing payload text into a
        // log line or a model-visible error.
        let mut body =
            "{\"method\":\"tools/call\",\"params\":{\"name\":\"translate\",\"text\":\"café"
                .as_bytes()
                .to_vec();
        body.push(0x07); // a raw BEL inside the string: the only defect
        body.extend_from_slice("\u{4e2d}\u{6587} \u{1f980}\"}}".as_bytes());

        let err = serde_json::from_slice::<Value>(&body)
            .expect_err("the BEL is refused; the multibyte text before it is legal");
        assert!(
            err.to_string().contains("control character"),
            "the failure is the control character, not the unicode: {err}"
        );
        let resp = parse_error_response(&body, &err, "srv")
            .expect("a brace-led JSON-RPC attempt is answered, not forwarded");
        let v = jsonrpc_of(resp).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert!(
            !message.contains("café") && !message.contains("中文") && !message.contains("🦀"),
            "payload text must not reach the answer, only its bytes: {message}"
        );
        assert!(
            message.len() < 450,
            "the full answer stays bounded: {message}"
        );
    }

    #[tokio::test]
    async fn a_multi_kb_body_still_gets_a_bounded_answer() {
        // A multi-kilobyte argument payload must not become a
        // multi-kilobyte error: the answer stays bounded no matter the body
        // size, nothing but the windowed bytes reaches it, and the offset
        // for an end-of-input failure clamps to the body's end.
        let filler = "7".repeat(12_000);
        let body = format!("{{\"method\":\"tools/call\",\"params\":{{\"note\":\"{filler}\"");

        let err = serde_json::from_slice::<Value>(body.as_bytes())
            .expect_err("an unterminated object does not parse");
        let resp = parse_error_response(body.as_bytes(), &err, "srv")
            .expect("a brace-led attempt is answered");
        let v = jsonrpc_of(resp).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert!(
            message.len() < 450,
            "12k of filler must not leak: {message}"
        );
        assert!(
            !message.contains(&"7".repeat(16)),
            "only the bounded window may appear, never bulk payload: {message}"
        );
        assert!(
            message.contains(&format!("offset {} |", body.len())),
            "an EOF failure points at the body's end: {message}"
        );
    }

    #[tokio::test]
    async fn the_offset_walks_to_a_failure_on_a_later_line() {
        // serde reports line/column; the diagnostic reports a whole-body
        // byte offset. A failure on line 2 must land past line 1's bytes,
        // with the newline itself visible in the dump window.
        let body = b"{\"a\":1,\n\"b\":2,,}";
        let err =
            serde_json::from_slice::<Value>(&body[..]).expect_err("a doubled comma is refused");
        let resp =
            parse_error_response(&body[..], &err, "srv").expect("a brace-led attempt is answered");
        let v = jsonrpc_of(resp).await;
        let message = v.pointer("/error/message").and_then(Value::as_str).unwrap();
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32700)));
        assert!(
            message.contains("offset 15 |"),
            "line 2 column 7 is byte 15 of the whole body: {message}"
        );
        assert!(
            message.contains(" 0a "),
            "the walked window crosses the newline: {message}"
        );
    }

    #[tokio::test]
    async fn a_batch_carrying_a_tools_call_is_refused_per_member_never_relayed() {
        // The finding's shape: an agent granted only ["echo"] hides
        // delete_repo inside a batch, which gate() never inspected.
        let batch = json!([
            { "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "delete_repo" } },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": "b", "method": "tools/list" },
        ]);
        let resp = batch_refusal(Some(&batch)).expect("the batch is refused, not relayed");
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_of(resp).await;
        let replies = v.as_array().expect("a batch answers a JSON array");
        assert_eq!(replies.len(), 2, "one reply per member with an id: {v}");
        assert_eq!(replies[0].pointer("/id"), Some(&json!(1)));
        assert_eq!(replies[1].pointer("/id"), Some(&json!("b")));
        for r in replies {
            assert_eq!(r.pointer("/error/code"), Some(&json!(-32600)));
            assert_eq!(
                r.pointer("/error/message").and_then(Value::as_str),
                Some(BATCH_REFUSED)
            );
        }
        // A batch of id-less calls still gets an answer.
        let silent =
            json!([{ "jsonrpc": "2.0", "method": "tools/call", "params": { "name": "x" } }]);
        let v = body_of(batch_refusal(Some(&silent)).unwrap()).await;
        assert_eq!(v.pointer("/error/code"), Some(&json!(-32600)));
        assert_eq!(v.pointer("/id"), Some(&Value::Null));
        // Chatter and single requests relay as before.
        let chatter = json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }]);
        assert!(batch_refusal(Some(&chatter)).is_none());
        let single = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "echo" } });
        assert!(
            batch_refusal(Some(&single)).is_none(),
            "a single call goes to the gate"
        );
        assert!(batch_refusal(None).is_none());
    }
}

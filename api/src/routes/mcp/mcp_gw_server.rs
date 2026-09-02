// /api/mcp/gw/{server}.
// The MCP gateway — the registry's ENFORCEMENT point. Agents never see an
// upstream URL or credential: their configs point here, the agent's own
// credential identifies the caller (agent-auth), and the gateway
//   · forwards JSON-RPC to the upstream (org headers, or the acting user's
//     connected-account headers on per-user servers)
//   · FILTERS tools/list down to the allowed set
//   · REJECTS tools/call outside it
// so a hand-edited agent config can never exceed what the registry granted.

use crate::agent_auth::{AgentSubject, presented, require_agent, subject_model};
use crate::error::{house_error, thrown_internal_error, upstream_error_message};
use crate::mcp::jsonrpc::rpc_error;
use crate::mcp::registry::{effective_mcp_for, parse_mcp_response};
use crate::state::AppState;
use crate::workspace_secrets::spend_handles_in_tool_call;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

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

pub async fn post(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Pass the CALLER, never `caller.model`. `subject_model`/`subject_proven`
    // read a bare string as PROVEN, so downgrading to the name here throws
    // away `legacy` — and this route is where that matters most: it resolves
    // the acting owner and can put that human's OAuth bearer token into
    // `upstreamHeaders`. `name` below is only ever used where a header or an
    // unthreaded callee genuinely needs the string.
    let subject = AgentSubject::Caller(caller.clone());
    let name = caller.model.clone();
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[mcp/gw] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let eff = match effective_mcp_for(&state.pg, &sb, &subject, &server_name).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[mcp/gw] effective resolution failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(eff) = eff else {
        return house_error(StatusCode::FORBIDDEN, "no access to this MCP server");
    };

    let mut body_text = String::from_utf8_lossy(&body).into_owned();
    let mut rpc: Option<Value> = serde_json::from_slice(&body).ok();
    // A non-JSON body (batch or ping) passes through untouched.

    // The call gate: reject disallowed tools before the upstream ever
    // hears about them.
    if let Some(resp) = gate(rpc.as_ref(), eff.tools.as_ref()) {
        return resp;
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
                tracing::error!("[mcp/gw] spend boundary failed: {e}");
                return thrown_internal_error();
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
        }
    }

    // The Workbench surface dispatches IN-PROCESS with the caller's agent
    // identity — grants resolved by the same gateway rules as any server.
    if eff.server.url.starts_with("talaria-workbench://") {
        let deps = crate::workbench::mcp::WorkbenchDeps {
            pg: state.pg.clone(),
            sb: sb.clone(),
            redis: state.redis().await.ok(),
        };
        let rpc_body = rpc.clone().unwrap_or_else(|| json!({}));
        let (status, out) = crate::workbench::mcp::dispatch_workbench_mcp(
            &deps,
            &rpc_body,
            &subject,
            eff.tools.as_deref(),
        )
        .await;
        return match out {
            None => (status, Body::empty()).into_response(),
            Some(out) => (status, Json(out)).into_response(),
        };
    }

    // App-published servers dispatch IN-PROCESS in TS through the app module
    // — authors' TS/node code, which rule 10 keeps on the TS side (the
    // never-port surface). `app-*` servers live there, not here; a direct
    // hit answers the boundary sentence instead of pretending to dispatch.
    if let Some(app_slug) = &eff.server.app_slug {
        return house_error(
            StatusCode::BAD_GATEWAY,
            &format!(
                "app \"{app_slug}\" dispatches in-process through the app runtime, which stays TS by rule 10 (docs/RUST-MIGRATION.md)"
            ),
        );
    }

    // The builtin toolkit is a child of THIS process, spawned on demand —
    // and a deploy's first agent session can beat the spawn. A session
    // whose initialize fails drops the server (and with it every talaria
    // tool) for its whole lifetime, so the relay heals the race instead of
    // reporting it: bring the child up and wait, then let the fetch below
    // speak for the outcome either way. A live child answers the probe in
    // milliseconds; only a dead one pays the wait.
    if eff.server.builtin {
        crate::mcp::service::await_mcp_service(8_000).await;
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
        && crate::safe_fetch::assert_fetchable_url(&eff.server.url)
            .await
            .is_err()
    {
        return house_error(
            StatusCode::BAD_GATEWAY,
            "upstream URL refused (not a reachable external address)",
        );
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
    let mut req = crate::gateway::provider::http()
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
            crate::error::log_upstream_error(
                &format!("mcp-gw:{server_name}"),
                "unreachable",
                &e.to_string(),
            );
            return house_error(StatusCode::BAD_GATEWAY, "upstream unreachable");
        }
    };
    relay(upstream, rpc.as_ref(), eff.tools.as_deref(), &server_name).await
}

/// GET — the streamable-HTTP notification stream (server → client): plain
/// relay, same gates as POST.
pub async fn get(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    // Same rule as POST: the caller carries the proof, the string does not.
    let subject = AgentSubject::Caller(caller);
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[mcp/gw] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let eff = match effective_mcp_for(&state.pg, &sb, &subject, &server_name).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[mcp/gw] effective resolution failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(eff) = eff else {
        return house_error(StatusCode::FORBIDDEN, "no access to this MCP server");
    };
    // App servers have no notification stream — decline politely.
    if eff.server.app_slug.is_some() {
        return (StatusCode::METHOD_NOT_ALLOWED, Body::empty()).into_response();
    }
    // Same heal as POST: the builtin child may not be up when the client
    // opens its notification stream.
    if eff.server.builtin {
        crate::mcp::service::await_mcp_service(8_000).await;
    }

    // Same rule as POST: validate a non-builtin upstream URL before the hop;
    // the response is a live SSE relay, so the fetch itself stays raw.
    if !eff.server.builtin
        && crate::safe_fetch::assert_fetchable_url(&eff.server.url)
            .await
            .is_err()
    {
        return house_error(
            StatusCode::BAD_GATEWAY,
            "upstream URL refused (not a reachable external address)",
        );
    }
    let hdr = |name: &'static str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let mut req = crate::gateway::provider::http().get(&eff.server.url);
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
            crate::error::log_upstream_error(
                &format!("mcp-gw-get:{server_name}"),
                "unreachable",
                &e.to_string(),
            );
            return house_error(StatusCode::BAD_GATEWAY, "upstream unreachable");
        }
    };
    // A failed notification-stream hop relays the same way as POST: fixed
    // sentence, verbatim to the log. Only a live 200 SSE stream passes.
    let status = upstream.status().as_u16();
    if !(200..300).contains(&status) {
        let text = upstream.text().await.unwrap_or_default();
        crate::error::log_upstream_error(&format!("mcp-gw-get:{server_name}"), status, &text);
        return house_error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            &upstream_error_message(status),
        );
    }
    let ct = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::OK))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| thrown_internal_error())
}

/// The POST relay tail: the HTTP-failure gate, then the tools/list filter
/// (JSON or SSE-framed), else the verbatim stream.
async fn relay(
    upstream: reqwest::Response,
    rpc: Option<&Value>,
    tools: Option<&[String]>,
    server_name: &str,
) -> Response {
    let status = upstream.status().as_u16();
    // HTTP-level failures never relay verbatim: their bodies are written by
    // whatever proxy or server answered, not by the MCP protocol. One check
    // here covers every relay below (tools/list filter, its fallback, the
    // final stream). JSON-RPC errors ride 200s and pass untouched — tool
    // results, including tool FAILURES, are the protocol the agent speaks.
    if !(200..300).contains(&status) {
        let text = upstream.text().await.unwrap_or_default();
        crate::error::log_upstream_error(&format!("mcp-gw:{server_name}"), status, &text);
        return house_error(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            &upstream_error_message(status),
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
            .unwrap_or_else(|_| thrown_internal_error());
    }
    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| thrown_internal_error())
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

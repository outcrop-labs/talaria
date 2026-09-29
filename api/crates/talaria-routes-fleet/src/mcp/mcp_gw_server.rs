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
// so a hand-edited agent config can never exceed what the registry granted.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::{AgentSubject, presented, require_agent, subject_model};
use talaria_api_facades::mcp::jsonrpc::rpc_error;
use talaria_api_facades::mcp::registry::{effective_mcp_for, parse_mcp_response};
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

pub async fn post(
    State(state): State<AppState>,
    Path(server_name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    // Pass the CALLER, never `caller.model`. `subject_model`/`subject_proven`
    // read a bare string as PROVEN, so downgrading to the name here throws
    // away `legacy` — and this route is where that matters most: it resolves
    // the acting owner and can put that human's OAuth bearer token into
    // `upstreamHeaders`. `name` below is only ever used where a header or an
    // unthreaded callee genuinely needs the string.
    let subject = AgentSubject::Caller(caller.clone());
    let name = caller.model.clone();
    let sb = secretbox_or_500(&state, "[mcp/gw] secretbox unavailable").await?;
    let eff = match effective_mcp_for(&state.pg, &sb, &subject, &server_name).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[mcp/gw] effective resolution failed", e)),
    };
    let Some(mut eff) = eff else {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "no access to this MCP server",
        ));
    };

    // A body that parses runs the gate and the spend boundary below. One
    // that doesn't still passes through verbatim — EXCEPT a brace-led body,
    // a JSON-RPC attempt with a syntax error in it, which must not skip
    // them: it is answered as a JSON-RPC parse error instead (above).
    let mut rpc: Option<Value> = match serde_json::from_slice(&body) {
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
            Err(e) => return Ok(internal("[mcp/gw] spend boundary failed", e)),
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
                    Err(e) => return Ok(house_error(StatusCode::BAD_GATEWAY, &e)),
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
                    Ok((status, out)) if out.is_null() => (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                        Body::empty(),
                    )
                        .into_response(),
                    Ok((status, out)) => (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                        Json(out),
                    )
                        .into_response(),
                    Err(e) => house_error(StatusCode::BAD_GATEWAY, &e),
                },
            );
        }
    }

    // App-published servers dispatch IN-PROCESS in TS through the app module
    // — authors' TS/node code, which rule 10 keeps on the TS side (the
    // never-port surface). `app-*` servers live there, not here; a direct
    // hit answers the boundary sentence instead of pretending to dispatch.
    if let Some(app_slug) = &eff.server.app_slug {
        return Ok(house_error(
            StatusCode::BAD_GATEWAY,
            &format!(
                "app \"{app_slug}\" dispatches in-process through the app runtime, which stays TS by rule 10 (docs/RUST-MIGRATION.md)"
            ),
        ));
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
        return Ok(house_error(
            StatusCode::BAD_GATEWAY,
            "upstream URL refused (not a reachable external address)",
        ));
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
            return Ok(house_error(StatusCode::BAD_GATEWAY, "upstream unreachable"));
        }
    };
    Ok(relay(upstream, rpc.as_ref(), eff.tools.as_deref(), &server_name).await)
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
    let eff = match effective_mcp_for(&state.pg, &sb, &subject, &server_name).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[mcp/gw] effective resolution failed", e)),
    };
    let Some(mut eff) = eff else {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "no access to this MCP server",
        ));
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
        talaria_error::log_upstream_error(&format!("mcp-gw:{server_name}"), status, &text);
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
            .unwrap_or_else(|e| internal("[fleet] response build failed", e));
    }
    builder
        .body(Body::from_stream(upstream.bytes_stream()))
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
}

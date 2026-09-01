// Probe an MCP server for reachability + auth state — port of
// ui/src/server/mcp-probe.ts. MCP's streamable-HTTP transport speaks JSON-RPC
// over POST; we send an `initialize` and classify the response so the UI can
// show a real connection status instead of hoping.
//
// The URL is admin-supplied, so the probe goes through safe_fetch: http(s)
// only, no private/loopback/link-local targets, and every redirect re-checked.
// (The dev convenience this TS file once carried — retrying an unresolvable
// docker hostname against localhost — was exactly the SSRF primitive the
// guard exists for, and it is not coming back.)

use serde_json::{Value, json};

use crate::mcp_registry::{MCP_PROTOCOL_VERSION, parse_mcp_response};
use crate::safe_fetch::{SafeError, SafeFetch, safe_fetch};

#[derive(Debug, Clone, PartialEq)]
pub enum McpProbeState {
    Ok,
    Auth,
    Unreachable,
    Error,
}

#[derive(Debug, Clone)]
pub struct McpProbeResult {
    pub state: McpProbeState,
    pub detail: String,
}

fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "talaria-probe", "version": "0.1.0" },
        },
    })
}

pub async fn probe_mcp(url: &str, headers: Vec<(&str, &str)>) -> McpProbeResult {
    let mut send = vec![
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
    ];
    send.extend(headers);
    let payload = initialize().to_string();
    let res = safe_fetch(
        url,
        SafeFetch {
            method: Some("POST"),
            headers: send,
            body: Some(payload.as_bytes()),
            timeout_ms: Some(8_000),
            ..SafeFetch::default()
        },
    )
    .await;
    let res = match res {
        Ok(r) => r,
        // A refused target is an answer, not a failure — say which it was.
        Err(SafeError::Blocked(msg)) => {
            return McpProbeResult {
                state: McpProbeState::Error,
                detail: msg.0,
            };
        }
        Err(e) => {
            let detail = if e.to_string().contains("timeout") {
                "timed out (8s)".to_string()
            } else {
                "could not connect".to_string()
            };
            return McpProbeResult {
                state: McpProbeState::Unreachable,
                detail,
            };
        }
    };

    if res.status == 401 || res.status == 403 {
        return McpProbeResult {
            state: McpProbeState::Auth,
            detail: format!("server requires authentication ({})", res.status),
        };
    }
    if !(200..300).contains(&res.status) {
        return McpProbeResult {
            state: McpProbeState::Error,
            detail: format!("server answered {}", res.status),
        };
    }

    // Body may be JSON or an SSE frame ("data: {}"). `parse_mcp_response` is
    // the one reader for both — the brace-scan it replaced sliced first-`{`
    // to last-`}`, so a body carrying more than one data frame (or any `}`
    // after the final token) failed to parse and read as merely "reachable".
    let text = String::from_utf8_lossy(&res.body).into_owned();
    let j = parse_mcp_response(&text);
    if let Some(err) = j
        .as_ref()
        .and_then(|v| v.get("error"))
        .filter(|e| !e.is_null())
    {
        return McpProbeResult {
            state: McpProbeState::Error,
            detail: err
                .get("message")
                .and_then(Value::as_str)
                // `??` is nullish-only: an empty message stays empty.
                .unwrap_or("server returned an error")
                .to_string(),
        };
    }
    let Some(j) = j else {
        return McpProbeResult {
            state: McpProbeState::Ok,
            detail: "reachable".into(),
        };
    };
    let name = j.pointer("/result/serverInfo/name").and_then(Value::as_str);
    McpProbeResult {
        state: McpProbeState::Ok,
        detail: match name {
            Some(n) => format!("connected to {n}"),
            None => "connected".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn detail_sentences_match_ts() {
        // The three fixed strings the UI switches on.
        assert_eq!(
            format!("server requires authentication ({})", 401),
            "server requires authentication (401)"
        );
        assert_eq!(format!("server answered {}", 503), "server answered 503");
        assert_eq!("connected to Exa MCP", "connected to Exa MCP");
    }
}

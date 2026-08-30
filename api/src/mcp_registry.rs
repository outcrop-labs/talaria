// The org-wide MCP registry — Composio's shape, self-hosted and tailored:
//   servers      registered once for the whole org (url + org headers)
//   assignments  which agents carry a server, optionally a TOOL SUBSET
//   user access  which users may exercise a server through agents acting for
//                them, optionally their own tool subset
//   credentials  per-user connected accounts: on auth_mode 'per-user' the
//                gateway injects the ACTING user's sealed headers, so the same
//                server acts as each user, never as a shared identity
// Enforcement is the /api/mcp/gw gateway, not config: agents only ever see
// gateway URLs, and the gateway filters tools/list + rejects tools/call outside
// the allowed set. Config can't be jailbroken past the registry.
//
// BOUNDED PORT: what crosses here is the ORG'S OWN DOOR — `getMcpServer` +
// `callMcpTool` + the one JSON-RPC conversation under them — because that is
// the edge the research tool-search transport needs. The admin CRUD, the
// assignment/access rows, per-user credentials, and the gateway's
// `effectiveMcpFor` brain stay TS until their routes cross (batch 5).
//
// THE CALLER EDGE TS HAS AND THIS DOES NOT: `callMcpTool` takes an optional
// `caller`, and a caller is the trigger for WORKSPACE-SECRET resolution — a
// model holding «secret:deploy.github_pat» spends it here, on its way out to a
// tool. The one ported caller (the research search step) passes NO caller, so
// the boundary is inert on this side and the parameter is omitted rather than
// accepted-and-ignored; it crosses with the first caller that has a user behind
// it, together with workspace-secrets itself.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::gateway::provider::http;
use crate::safe_fetch::{SafeFetch, safe_fetch};

/// The protocol revision Talaria speaks at the MCP handshake. TS home:
/// ui/src/server/mcp-protocol.ts.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// A registry row, bounded to the fields a call needs (the TS `McpServer`
/// carries the admin surface too — tools, required headers, labels — which
/// cross with the MCP routes that read them).
pub struct McpServerRow {
    pub id: String,
    pub name: String,
    pub url: String,
    /// PLAINTEXT jsonb — org-level headers an admin typed, sealed nowhere.
    pub headers: Map<String, Value>,
    pub timeout_secs: Option<i64>,
    pub enabled: bool,
    /// 'org' = shared org headers; 'per-user' = each user connects their own.
    pub auth_mode: String,
    /// The server negotiates OAuth (discovered from its 401 challenge).
    pub oauth_enabled: bool,
    /// Talaria's own toolkit — governable here, but not removable/reconfigurable.
    pub builtin: bool,
    /// Set when a Talaria app publishes this server — its calls dispatch
    /// in-process, which no Rust stage can do.
    pub app_slug: Option<String>,
}

/// `getMcpServer`: id OR name, one row.
pub async fn get_mcp_server(
    pg: &PgPool,
    id_or_name: &str,
) -> Result<Option<McpServerRow>, sqlx::Error> {
    type Row = (
        String,         // id
        String,         // name
        String,         // url
        Value,          // headers
        Option<i64>,    // timeout_secs
        bool,           // enabled
        String,         // auth_mode
        bool,           // oauth_enabled
        bool,           // builtin
        Option<String>, // app_slug
    );
    let row: Option<Row> = sqlx::query_as(
        "select id::text, name, url, headers, timeout_secs, enabled, auth_mode::text, \
                (oauth is not null) as oauth_enabled, builtin, app_slug::text \
         from mcp_servers where id::text = $1 or name = $1",
    )
    .bind(id_or_name)
    .fetch_optional(pg)
    .await?;
    let Some((
        id,
        name,
        url,
        headers,
        timeout_secs,
        enabled,
        auth_mode,
        oauth_enabled,
        builtin,
        app_slug,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(McpServerRow {
        id,
        name,
        url,
        headers: headers.as_object().cloned().unwrap_or_default(),
        timeout_secs,
        enabled,
        auth_mode,
        oauth_enabled,
        builtin,
        app_slug,
    }))
}

/// One tool result, flattened to the two things a caller needs: the text the
/// model should see, and whatever structured payload the server also returned.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolResult {
    /// Every text content block, joined. Empty when the server returned none.
    pub text: String,
    /// `structuredContent`, verbatim, when the server sent it.
    pub structured: Option<Value>,
    pub is_error: bool,
}

/// CALL ONE TOOL ON A REGISTERED SERVER, AS THE ORG.
///
/// This is the platform's own door to MCP, and it is deliberately NOT the
/// agent-facing one. Agents reach tools through `/api/mcp/gw/<server>`, which
/// resolves the acting agent, intersects its assignment with the owner's
/// allowance, and injects per-user credentials — none of which applies to a
/// platform stage like the research search step, where there is no agent and no
/// acting human, only Talaria doing a job the org configured it to do.
///
/// REFUSES PER-USER SERVERS, and that refusal is the security-relevant line in
/// this function. On `auth_mode: 'per-user'` the credentials belong to a HUMAN;
/// a platform caller has no such human, and quietly using the org headers
/// instead would act as a shared identity on a server explicitly configured to
/// never be one. Better to fail and say so.
///
/// AND REFUSES TWO MORE EDGES THE TS SIDE DISPATCHES IN-PROCESS, each named so
/// the failure reads as a port gap rather than a misconfigured registry: an
/// APP-PUBLISHED server dispatches through the compiled app module, and an
/// OAUTH server's org bearer lives sealed in `mcp_oauth_tokens` with a refresh
/// leg. Both cross in the batch-5 fleet/MCP plane; both are ledger items in
/// RUST-MIGRATION.md until then.
pub async fn call_mcp_tool(
    pg: &PgPool,
    server_name: &str,
    tool: &str,
    args: &Map<String, Value>,
) -> Result<McpToolResult, String> {
    let Some(server) = get_mcp_server(pg, server_name)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err(format!("MCP server \"{server_name}\" is not registered"));
    };
    if !server.enabled {
        return Err(format!("MCP server \"{server_name}\" is disabled"));
    }
    if server.auth_mode == "per-user" {
        return Err(format!(
            "MCP server \"{server_name}\" authenticates per user, so a platform stage cannot \
             call it. Register an org-authenticated server for this."
        ));
    }
    if let Some(slug) = &server.app_slug {
        return Err(format!(
            "MCP server \"{server_name}\" is published by the app \"{slug}\" and dispatches \
             in-process through the app runtime, which has not crossed to Rust yet"
        ));
    }
    if server.oauth_enabled {
        return Err(format!(
            "MCP server \"{server_name}\" negotiates OAuth, and its org token lives sealed in \
             mcp_oauth_tokens, which has not crossed to Rust yet"
        ));
    }

    let init = org_call(
        &server,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "talaria", "version": "1.0" },
            },
        }),
        None,
    )
    .await?;
    if init.status >= 400 {
        return Err(format!(
            "MCP server \"{server_name}\" answered {}",
            init.status
        ));
    }
    let res = org_call(
        &server,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": args },
        }),
        init.session.as_deref(),
    )
    .await?;
    if res.status >= 400 {
        return Err(format!(
            "MCP tool \"{server_name}.{tool}\" answered {}",
            res.status
        ));
    }
    read_tool_result(res.json, server_name, tool)
}

/// ONE JSON-RPC CONVERSATION WITH A SERVER, AS THE ORG — the `orgSession`
/// half. PLATFORM-INITIATED by construction: Talaria asking a server something
/// on the org's behalf, not an agent acting for a person, which is why the org
/// headers are right here and why nothing in this helper takes a user.
struct OrgReply {
    json: Option<Value>,
    session: Option<String>,
    status: u16,
}

/// One POST of the conversation: org headers plus session, JSON or SSE-framed
/// reply parsed, the session header carried forward.
async fn org_call(
    server: &McpServerRow,
    body: &Value,
    session_id: Option<&str>,
) -> Result<OrgReply, String> {
    // Header assembly in the TS order — content-type, accept, org headers,
    // builtin identity, session — so a later spread overriding an earlier key
    // (a session header named in `headers`, say) overrides the same way.
    let mut pairs: Vec<(String, String)> = vec![
        ("content-type".into(), "application/json".into()),
        (
            "accept".into(),
            "application/json, text/event-stream".into(),
        ),
    ];
    for (k, v) in &server.headers {
        if let Some(v) = v.as_str() {
            pairs.push((k.clone(), v.to_string()));
        }
    }
    if server.builtin {
        // The builtin toolkit authenticates with the fleet key as Talaria
        // itself (an empty key when the env is unset is the TS spelling).
        pairs.push(("X-Agent-Name".into(), "talaria".into()));
        pairs.push((
            "X-Api-Key".into(),
            std::env::var("TALARIA_AGENT_KEY").unwrap_or_default(),
        ));
    }
    if let Some(session) = session_id {
        pairs.push(("mcp-session-id".into(), session.to_string()));
    }
    let payload = serde_json::to_string(body).expect("the conversation body is plain data");
    let timeout_ms = Duration::from_millis(server.timeout_secs.unwrap_or(30).unsigned_abs() * 1000);

    // Everything but the built-in toolkit is a URL (and a header set) someone
    // with agents.manage typed — straight through the SSRF guard. The builtin
    // row is Talaria's own MCP service on loopback, written by the TS
    // `ensureBuiltinMcp` and un-editable by design (`updateMcpServer` refuses
    // url/headers patches on builtin), so it is infrastructure, not input.
    let (status, headers, text) = if server.builtin {
        let mut req = http().post(&server.url).timeout(timeout_ms);
        for (k, v) in &pairs {
            req = req.header(k, v);
        }
        let res = req.body(payload).send().await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let text = res.text().await.map_err(|e| e.to_string())?;
        (status, headers, text)
    } else {
        let borrowed: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let res = safe_fetch(
            &server.url,
            SafeFetch {
                method: Some("POST"),
                headers: borrowed,
                body: Some(payload.as_bytes()),
                timeout_ms: Some(timeout_ms.as_millis() as u64),
                ..SafeFetch::default()
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        (
            res.status,
            res.headers,
            String::from_utf8_lossy(&res.body).into_owned(),
        )
    };
    let session = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    Ok(OrgReply {
        json: parse_mcp_response(&text),
        session,
        status,
    })
}

/// The MCP `tools/call` result shape, flattened. One reader for every call
/// path, so no two of them can disagree about what a tool returned.
fn read_tool_result(
    raw: Option<Value>,
    server_name: &str,
    tool: &str,
) -> Result<McpToolResult, String> {
    let body = raw.as_ref().and_then(Value::as_object);
    // TS `if (body?.error)` — truthy: a present non-null error object fails the
    // call even when the HTTP layer said 200.
    if let Some(err) = body.and_then(|b| b.get("error")).filter(|e| !e.is_null()) {
        // `message ?? 'unknown error'` — nullish only, so an EMPTY message is
        // itself, not "unknown".
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("MCP tool \"{server_name}.{tool}\" failed: {msg}"));
    }
    let result = body
        .and_then(|b| b.get("result"))
        .and_then(Value::as_object);
    let text = result
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let o = c.as_object()?;
            if o.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            o.get("text").and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    Ok(McpToolResult {
        text,
        structured: result
            .and_then(|r| r.get("structuredContent"))
            .cloned()
            .filter(|v| !v.is_null()),
        is_error: result.and_then(|r| r.get("isError")) == Some(&Value::Bool(true)),
    })
}

/// JSON body, or the LAST parseable data frame of an SSE-encoded response.
pub fn parse_mcp_response(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with('{') || t.starts_with('[') {
        return serde_json::from_str(t).ok();
    }
    // `slice(5)` is safe on a `data:`-prefixed line even mid-codepoint, and
    // `data:` alone parses to nothing, so the frame is simply skipped.
    let frames: Vec<&str> = t
        .lines()
        .filter(|l| l.starts_with("data:"))
        .map(|l| l[5..].trim())
        .collect();
    for f in frames.into_iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(f) {
            return Some(v);
        }
    }
    None
}

// ── serversForAgent — the render's registry view ─────────────────────────────
// (mcp-registry.ts serversForAgent + the three row checks it consults.) This
// one legitimately answers about a THIRD PARTY — fleet-render and the /api/mcp
// admin listing ask "what should <model> carry?" with no caller in hand — so
// a bare model string stays accepted. It grants nothing on its own: the
// credential is never rendered, only a gateway URL, and the gateway re-derives
// access per request through `effectiveMcpFor`.

/// Does this server have OAuth tokens for a subject ('org' or a user id)?
async fn has_oauth_tokens(
    pg: &PgPool,
    server_id: &str,
    subject: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from mcp_oauth_tokens where server_id::text = $1 and subject = $2",
    )
    .bind(server_id)
    .bind(subject)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// Does this server have per-user credentials stored for a user?
async fn has_user_credentials(
    pg: &PgPool,
    server_id: &str,
    user_id: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from mcp_user_credentials where server_id::text = $1 and user_id::text = $2",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// model → owner_user_id for every PERSONAL assistant (users.ts
/// personalAssistantOwners). The render passes a bare model string — proven
/// by construction there — so the owner map is the whole lookup.
async fn personal_assistant_owners(pg: &PgPool) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select model, owner_user_id::text from agent_defs where owner_user_id is not null",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().collect())
}

#[derive(Debug, Clone)]
pub struct AgentServer {
    pub name: String,
    pub timeout_secs: Option<i64>,
}

/// Every server an agent should carry in its rendered config (gateway URLs).
pub async fn servers_for_agent(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Vec<AgentServer>, sqlx::Error> {
    // (name, timeout_secs, auth_mode, oauth_enabled, id)
    let rows: Vec<(String, Option<i64>, String, bool, String)> = sqlx::query_as(
        "select s.name, s.timeout_secs::int8, s.auth_mode, (s.oauth is not null), s.id::text \
         from mcp_servers s \
         where s.enabled and not s.builtin and (s.all_agents or exists ( \
           select 1 from mcp_server_agents a where a.server_id = s.id and a.agent_model = $1 \
         )) \
         order by s.name",
    )
    .bind(agent_model)
    .fetch_all(pg)
    .await?;
    let owner = personal_assistant_owners(pg)
        .await?
        .get(agent_model)
        .cloned();
    let mut out = Vec::new();
    for (name, timeout_secs, auth_mode, oauth_enabled, id) in rows {
        if auth_mode == "per-user" {
            // Only rendered once the acting user actually connected an account.
            let Some(owner) = &owner else {
                continue;
            };
            let connected = if oauth_enabled {
                has_oauth_tokens(pg, &id, owner).await?
            } else {
                has_user_credentials(pg, &id, owner).await?
            };
            if !connected {
                continue;
            }
        } else if oauth_enabled && !has_oauth_tokens(pg, &id, "org").await? {
            continue; // org account not connected yet — keep it out of configs
        } else if let Some(owner) = &owner {
            // A PA skips servers its owner is explicitly denied.
            let access: Option<(bool,)> = sqlx::query_as(
                "select allowed from mcp_user_access where server_id::text = $1 and user_id::text = $2",
            )
            .bind(&id)
            .bind(owner)
            .fetch_optional(pg)
            .await?;
            if access.is_some_and(|(allowed,)| !allowed) {
                continue;
            }
        }
        out.push(AgentServer { name, timeout_secs });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_reads_json_objects_arrays_and_the_last_sse_frame() {
        assert_eq!(
            parse_mcp_response(r#"  {"ok":true}  "#),
            Some(json!({ "ok": true }))
        );
        assert_eq!(parse_mcp_response("[1,2]"), Some(json!([1, 2])));
        // SSE: event framing around the payload, last parseable data frame
        // wins, unparsable frames are skipped on the way there.
        let sse = "event: message\ndata: not json\ndata: {\"a\":1}\ndata: {\"b\":2}\n\n";
        assert_eq!(parse_mcp_response(sse), Some(json!({ "b": 2 })));
        // `data:` with nothing after it is a frame that parses to nothing.
        let only_empty = "data:\ndata: {}";
        assert_eq!(parse_mcp_response(only_empty), Some(json!({})));
        // Nothing usable at all.
        assert_eq!(parse_mcp_response(""), None);
        assert_eq!(parse_mcp_response("   "), None);
        assert_eq!(
            parse_mcp_response("event: message\ndata: still not json"),
            None
        );
        assert_eq!(parse_mcp_response("plain text"), None);
    }

    #[test]
    fn read_tool_result_flattens_the_content_blocks() {
        let raw = Some(json!({
            "result": {
                "content": [
                    { "type": "text", "text": "first" },
                    { "type": "image", "data": "…" },
                    { "type": "text", "text": "second" },
                    { "type": "text", "text": 7 }
                ],
                "structuredContent": { "results": [1] },
                "isError": false
            }
        }));
        let got = read_tool_result(raw, "exa", "search").unwrap();
        assert_eq!(got.text, "first\nsecond");
        assert_eq!(got.structured, Some(json!({ "results": [1] })));
        assert!(!got.is_error);

        // Absent everything: empty text, null structured, not an error.
        let got = read_tool_result(Some(json!({ "result": {} })), "exa", "search").unwrap();
        assert_eq!(got.text, "");
        assert_eq!(got.structured, None);
        assert!(!got.is_error);
        let got = read_tool_result(None, "exa", "search").unwrap();
        assert_eq!(got.text, "");

        // isError is `=== true`, not truthy.
        let got = read_tool_result(
            Some(json!({ "result": { "isError": "yes" } })),
            "exa",
            "search",
        )
        .unwrap();
        assert!(!got.is_error);
        let got = read_tool_result(
            Some(json!({ "result": { "isError": true } })),
            "exa",
            "search",
        )
        .unwrap();
        assert!(got.is_error);
    }

    #[test]
    fn a_jsonrpc_error_fails_the_call_with_its_message() {
        let err = read_tool_result(
            Some(json!({ "error": { "message": "tool not found" } })),
            "exa",
            "search",
        )
        .unwrap_err();
        assert_eq!(err, "MCP tool \"exa.search\" failed: tool not found");
        // No message at all still names the tool.
        let err = read_tool_result(Some(json!({ "error": {} })), "exa", "search").unwrap_err();
        assert_eq!(err, "MCP tool \"exa.search\" failed: unknown error");
        // `??`, not truthiness: an empty message is itself.
        let err = read_tool_result(Some(json!({ "error": { "message": "" } })), "exa", "search")
            .unwrap_err();
        assert_eq!(err, "MCP tool \"exa.search\" failed: ");
    }
}

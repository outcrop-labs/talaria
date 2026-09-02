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
// IDENTITY: "the acting user" here is a HUMAN, and on a per-user server this
// module hands out that human's connected credentials (an OAuth bearer token,
// verbatim, in upstream_headers) — owner-proxying, resolved through
// `assistant_owner_for(subject)`, which refuses a legacy shared-key caller and
// never through the bare model map, whose string key reads as proven.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::PgPool;

use crate::agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model, subject_proven};
use crate::gateway::provider::http;
use crate::safe_fetch::{SafeFetch, safe_fetch};
use crate::secretbox::SecretBox;

/// The protocol revision Talaria speaks at the MCP handshake — one revision
/// for both directions of the conversation (what our dispatchers answer and
/// what every client asks). The literal lives in mcp::jsonrpc (the leaf
/// every dispatcher shares); re-exported here for the importers that reach
/// it via the registry.
pub use crate::mcp::jsonrpc::MCP_PROTOCOL_VERSION;

/// The column list every full-row read spells — the wire's field order IS
/// this order.
const ROW: &str = "id::text, name, label, description, url, headers, timeout_secs::int8, enabled, \
                   all_agents, auth_mode::text, tools, \
                   (trunc(extract(epoch from tools_refreshed_at) * 1000))::bigint as tools_refreshed_ms, \
                   required_headers, (oauth is not null) as oauth_enabled, builtin, \
                   app_slug::text, created_by, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms";

/// A registry row, every column. The jsonb columns ride
/// as the raw Value — pg owns their key order and the wire answer must carry
/// it byte-for-byte.
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub label: String,
    pub description: Option<String>,
    pub url: String,
    /// PLAINTEXT jsonb — org-level headers an admin typed, sealed nowhere.
    pub headers: Map<String, Value>,
    pub timeout_secs: Option<i64>,
    pub enabled: bool,
    pub all_agents: bool,
    /// 'org' = shared org headers; 'per-user' = each user connects their own.
    pub auth_mode: String,
    /// Cached tool catalog (refresh_mcp_tools writes it).
    pub tools: Value,
    pub tools_refreshed_ms: Option<i64>,
    /// Header declarations captured at install (drive per-user connect forms).
    pub required_headers: Value,
    /// The server negotiates OAuth (discovered from its 401 challenge).
    pub oauth_enabled: bool,
    /// Talaria's own toolkit — governable here, but not removable/reconfigurable.
    pub builtin: bool,
    /// Set when a Talaria APP publishes this server — its calls dispatch
    /// in-process on the TS side (app modules stay TS/node).
    pub app_slug: Option<String>,
    pub created_by: Option<String>,
    pub created_ms: i64,
}

/// A registry row as it comes off the wire, in ROW order. A derived struct,
/// not a tuple — sqlx stops implementing FromRow for tuples at 16 elements
/// and this row carries 18.
#[derive(sqlx::FromRow)]
struct ServerRow {
    id: String,
    name: String,
    label: String,
    description: Option<String>,
    url: String,
    headers: Value,
    timeout_secs: Option<i64>,
    enabled: bool,
    all_agents: bool,
    auth_mode: String,
    tools: Value,
    tools_refreshed_ms: Option<i64>,
    required_headers: Value,
    oauth_enabled: bool,
    builtin: bool,
    app_slug: Option<String>,
    created_by: Option<String>,
    created_ms: i64,
}

fn server_of_row(r: ServerRow) -> McpServer {
    McpServer {
        id: r.id,
        name: r.name,
        label: r.label,
        description: r.description,
        url: r.url,
        headers: r.headers.as_object().cloned().unwrap_or_default(),
        timeout_secs: r.timeout_secs,
        enabled: r.enabled,
        all_agents: r.all_agents,
        auth_mode: r.auth_mode,
        tools: r.tools,
        tools_refreshed_ms: r.tools_refreshed_ms,
        required_headers: r.required_headers,
        oauth_enabled: r.oauth_enabled,
        builtin: r.builtin,
        app_slug: r.app_slug,
        created_by: r.created_by,
        created_ms: r.created_ms,
    }
}

/// The row as the routes answer it — the ROW column order, timestamps as the
/// ISO strings `JSON.stringify(new Date(...))` produces.
pub fn server_wire(s: &McpServer) -> Value {
    serde_json::json!({
        "id": s.id,
        "name": s.name,
        "label": s.label,
        "description": s.description,
        "url": s.url,
        "headers": Value::Object(s.headers.clone()),
        "timeoutSecs": s.timeout_secs,
        "enabled": s.enabled,
        "allAgents": s.all_agents,
        "authMode": s.auth_mode,
        "tools": s.tools,
        "toolsRefreshedAt": s.tools_refreshed_ms.map(epoch_ms_to_iso),
        "requiredHeaders": s.required_headers,
        "oauthEnabled": s.oauth_enabled,
        "builtin": s.builtin,
        "appSlug": s.app_slug,
        "createdBy": s.created_by,
        "createdAt": epoch_ms_to_iso(s.created_ms),
    })
}

/// By id OR name, one row.
pub async fn get_mcp_server(
    pg: &PgPool,
    id_or_name: &str,
) -> Result<Option<McpServer>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's ROW column list.
    let sql = format!("select {ROW} from mcp_servers where id::text = $1 or name = $1");
    let row: Option<ServerRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id_or_name)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(server_of_row))
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
/// AND REFUSES ONE EDGE DISPATCHED IN-PROCESS: an APP-PUBLISHED server
/// dispatches through the compiled app module (apps/*/mcp.ts — authors'
/// TS/node code, owned by the app runtime). OAuth servers do not refuse
/// here — the org bearer rides `org_session`.
pub async fn call_mcp_tool(
    pg: &PgPool,
    sb: &SecretBox,
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
             in-process through the app runtime, which stays TS by rule 10 (docs/RUST-MIGRATION.md)"
        ));
    }

    let bearer = if server.oauth_enabled {
        crate::mcp::oauth::oauth_token_for(pg, sb, &server.id, "org").await?
    } else {
        None
    };
    let init = org_call(
        &server,
        bearer.as_deref(),
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
        bearer.as_deref(),
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

/// One POST of the conversation: org headers plus session plus (on OAuth
/// servers) the org bearer, JSON or SSE-framed reply parsed, the session
/// header carried forward.
/// Rejection sentences in undici's shape, because these exact strings are
/// the surface callers see: transport failures read bare "fetch failed"
/// (the cause chain carries the detail, the message does not) and a timeout
/// expiry reads "The operation was aborted due to timeout".
pub(crate) fn undici_message(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "The operation was aborted due to timeout".into()
    } else {
        "fetch failed".into()
    }
}

/// The same shapes for the safe-fetch leg — except a blocked URL, whose
/// refusal sentence is itself the message that surfaces, so it passes
/// through verbatim.
fn undici_safe_message(e: crate::safe_fetch::SafeError) -> String {
    match e {
        crate::safe_fetch::SafeError::Timeout => "The operation was aborted due to timeout".into(),
        crate::safe_fetch::SafeError::Blocked(b) => b.to_string(),
        crate::safe_fetch::SafeError::Fetch(_) => "fetch failed".into(),
    }
}

async fn org_call(
    server: &McpServer,
    bearer: Option<&str>,
    body: &Value,
    session_id: Option<&str>,
) -> Result<OrgReply, String> {
    // Header assembly order — content-type, accept, org headers, builtin
    // identity, bearer, session — so a later entry overriding an earlier key
    // (a session header named in `headers`, say) wins the same way.
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
        // itself — an empty key when the env is unset, not an absent header.
        pairs.push(("X-Agent-Name".into(), "talaria".into()));
        pairs.push((
            "X-Api-Key".into(),
            std::env::var("TALARIA_AGENT_KEY").unwrap_or_default(),
        ));
    }
    if let Some(bearer) = bearer {
        pairs.push(("authorization".into(), format!("Bearer {bearer}")));
    }
    if let Some(session) = session_id {
        pairs.push(("mcp-session-id".into(), session.to_string()));
    }
    let payload = serde_json::to_string(body).expect("the conversation body is plain data");
    let timeout_ms = Duration::from_millis(server.timeout_secs.unwrap_or(30).unsigned_abs() * 1000);

    // Everything but the built-in toolkit is a URL (and a header set) someone
    // with agents.manage typed — straight through the SSRF guard. The builtin
    // row is Talaria's own MCP service on loopback, written by
    // `ensure_builtin_mcp` and un-editable by design (`update_mcp_server`
    // refuses url/headers patches on builtin), so it is infrastructure, not
    // input.
    let (status, headers, text) = if server.builtin {
        let mut req = http().post(&server.url).timeout(timeout_ms);
        for (k, v) in &pairs {
            req = req.header(k, v);
        }
        let res = req
            .body(payload)
            .send()
            .await
            .map_err(|e| undici_message(&e))?;
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let text = res.text().await.map_err(|e| undici_message(&e))?;
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
        .map_err(undici_safe_message)?;
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
    // A present non-null error object fails the call even when the HTTP
    // layer said 200.
    if let Some(err) = body.and_then(|b| b.get("error")).filter(|e| !e.is_null()) {
        // Nullish-only default — an EMPTY message is itself, not "unknown".
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

// ── The registry write half ─────────────────────────────────────────────────

/// The Talaria toolkit as a governable system row: every agent carries it,
/// and the same per-agent/per-person tool subsets apply — enforced by the
/// gateway like any other server. Identity/lifecycle stay locked.
pub async fn ensure_builtin_mcp(pg: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into mcp_servers (name, label, description, url, all_agents, builtin, created_by) \
         values ('talaria', 'Talaria toolkit', \
                 'Talaria''s own tools: tickets, documents, knowledge, channels, research, media.', \
                 $1, true, true, 'talaria') \
         on conflict (name) do update set builtin = true, all_agents = true, enabled = true",
    )
    .bind(format!("http://127.0.0.1:{}/mcp", crate::mcp::service::mcp_port()))
    .execute(pg)
    .await?;
    // The Workbench surface — in-process like app servers, but NOT all_agents:
    // access is an explicit per-agent grant like any other governed capability.
    let tools = workbench_catalog();
    sqlx::query(
        "insert into mcp_servers (name, label, description, url, all_agents, created_by, tools, tools_refreshed_at) \
         values ('workbench', 'Workbench', \
                 'Sandboxed execution for granted agents: jobs, branches, and PRs under the platform-owned git flow.', \
                 'talaria-workbench://core', false, 'talaria', $1, now()) \
         on conflict (name) do update set tools = $1, tools_refreshed_at = now(), enabled = true",
    )
    .bind(&tools)
    .execute(pg)
    .await?;
    Ok(())
}

/// The Workbench tool catalog as the registry caches it — name + description
/// capped at 300.
fn workbench_catalog() -> Value {
    Value::Array(
        crate::workbench::mcp::workbench_tools()
            .into_iter()
            .map(|t| {
                let mut e = Map::new();
                e.insert(
                    "name".into(),
                    Value::String(
                        t.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                );
                if let Some(d) = t.get("description").and_then(Value::as_str) {
                    e.insert("description".into(), Value::String(utf16_slice(d, 300)));
                }
                Value::Object(e)
            })
            .collect(),
    )
}

/// JS `String.prototype.slice(0, n)` — UTF-16 code units, not chars.
fn utf16_slice(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0usize;
    for c in s.chars() {
        units += c.len_utf16();
        if units > n {
            return out;
        }
        out.push(c);
    }
    out
}

pub async fn list_mcp_servers(pg: &PgPool) -> Result<Vec<McpServer>, sqlx::Error> {
    let _ = ensure_builtin_mcp(pg).await; // best-effort — a failed ensure must not fail the list
    // AssertSqlSafe: the interpolation is this crate's ROW column list.
    let sql = format!("select {ROW} from mcp_servers order by builtin desc, name");
    let rows: Vec<ServerRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(server_of_row).collect())
}

/// Input to [`create_mcp_server`] — the parsed POST body, schema order.
pub struct NewServer<'a> {
    pub name: &'a str,
    pub label: Option<&'a str>,
    pub description: Option<&'a str>,
    pub url: &'a str,
    pub headers: Option<&'a Map<String, Value>>,
    pub timeout_secs: Option<i64>,
    pub auth_mode: &'a str,
    /// Declared header forms; defaults fill in per the schema's normalization.
    pub required_headers: &'a Value,
    pub created_by: &'a str,
}

pub async fn create_mcp_server(pg: &PgPool, input: &NewServer<'_>) -> Result<McpServer, String> {
    // name/description/isSecret/placeholder, with the nullish defaults
    // baked in.
    let declared: Vec<Value> = input
        .required_headers
        .as_array()
        .map(|a| {
            a.iter()
                .map(|h| {
                    serde_json::json!({
                        "name": h.get("name").cloned().unwrap_or(Value::Null),
                        "description": h.get("description").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        // `h.isSecret ?? false` — absent AND null both land on false.
                        "isSecret": h.get("isSecret").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Bool(false)),
                        "placeholder": h.get("placeholder").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // AssertSqlSafe: the interpolation is this crate's ROW column list.
    let sql = format!(
        "insert into mcp_servers (name, label, description, url, headers, timeout_secs, auth_mode, required_headers, created_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9) returning {ROW}"
    );
    let row: Option<ServerRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(input.name)
        .bind(input.label.unwrap_or(input.name))
        .bind(input.description)
        .bind(input.url)
        .bind(serde_json::Value::Object(
            input.headers.cloned().unwrap_or_default(),
        ))
        .bind(input.timeout_secs)
        .bind(input.auth_mode)
        .bind(Value::Array(declared))
        .bind(input.created_by)
        .fetch_optional(pg)
        .await
        .map_err(|e| e.to_string())?;
    row.map(server_of_row)
        .ok_or_else(|| "insert returned no row".to_string())
}

/// The updatable config fields — `None` = key absent from the patch. The
/// nullish fields (`description`, `timeoutSecs`) are `Option<Option<_>>`:
/// sending `null` CLEARS the column, which is a different request from
/// omitting the key.
#[derive(Default)]
pub struct ServerPatch {
    pub label: Option<String>,
    pub description: Option<Option<String>>,
    pub url: Option<String>,
    pub headers: Option<Map<String, Value>>,
    pub timeout_secs: Option<Option<i64>>,
    pub enabled: Option<bool>,
    pub all_agents: Option<bool>,
    pub auth_mode: Option<String>,
}

pub async fn update_mcp_server(pg: &PgPool, id: &str, patch: &ServerPatch) -> Result<(), String> {
    let row: Option<(bool, Option<String>)> =
        sqlx::query_as("select builtin, app_slug::text from mcp_servers where id::text = $1")
            .bind(id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    let Some((builtin, app_slug)) = row else {
        return Ok(()); // updates below match zero rows — a no-op
    };
    if builtin
        && (patch.url.is_some()
            || patch.headers.is_some()
            || patch.enabled.is_some()
            || patch.all_agents.is_some()
            || patch.auth_mode.is_some())
    {
        // The toolkit's identity and lifecycle are Talaria's; only access
        // rules (assignments/user access, handled elsewhere) are governable.
        return Err("the built-in Talaria toolkit cannot be reconfigured".into());
    }
    if app_slug.is_some()
        && (patch.url.is_some()
            || patch.headers.is_some()
            || patch.enabled.is_some()
            || patch.auth_mode.is_some())
    {
        // App servers have no upstream to point elsewhere and follow the app's
        // lifecycle; allAgents/access stay governable like any server.
        return Err("this server is published by an app; disable the app instead".into());
    }
    if let Some(label) = &patch.label {
        sqlx::query("update mcp_servers set label = $2, updated_at = now() where id::text = $1")
            .bind(id)
            .bind(label)
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(description) = &patch.description {
        sqlx::query(
            "update mcp_servers set description = $2, updated_at = now() where id::text = $1",
        )
        .bind(id)
        .bind(description)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    }
    if let Some(timeout_secs) = &patch.timeout_secs {
        sqlx::query(
            "update mcp_servers set timeout_secs = $2, updated_at = now() where id::text = $1",
        )
        .bind(id)
        .bind(timeout_secs)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    }
    if let Some(url) = &patch.url {
        // A repointed server's cached catalog is stale by definition.
        sqlx::query("update mcp_servers set url = $2, tools = '[]', tools_refreshed_at = null, updated_at = now() where id::text = $1")
            .bind(id).bind(url).execute(pg).await.map_err(|e| e.to_string())?;
    }
    if let Some(headers) = &patch.headers {
        sqlx::query("update mcp_servers set headers = $2, updated_at = now() where id::text = $1")
            .bind(id)
            .bind(Value::Object(headers.clone()))
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(enabled) = patch.enabled {
        sqlx::query("update mcp_servers set enabled = $2, updated_at = now() where id::text = $1")
            .bind(id)
            .bind(enabled)
            .execute(pg)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(all_agents) = patch.all_agents {
        sqlx::query(
            "update mcp_servers set all_agents = $2, updated_at = now() where id::text = $1",
        )
        .bind(id)
        .bind(all_agents)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    }
    if let Some(auth_mode) = &patch.auth_mode {
        sqlx::query(
            "update mcp_servers set auth_mode = $2, updated_at = now() where id::text = $1",
        )
        .bind(id)
        .bind(auth_mode)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub async fn delete_mcp_server(pg: &PgPool, id: &str) -> Result<(), String> {
    let row: Option<(bool, Option<String>)> =
        sqlx::query_as("select builtin, app_slug::text from mcp_servers where id::text = $1")
            .bind(id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    let Some((builtin, app_slug)) = row else {
        return Ok(()); // delete below matches zero rows — a no-op
    };
    if builtin {
        return Err("the built-in Talaria toolkit cannot be removed".into());
    }
    if app_slug.is_some() {
        return Err(
            "this server is published by an app; disable or uninstall the app instead".into(),
        );
    }
    sqlx::query("delete from mcp_servers where id::text = $1") // assignments/access/credentials cascade
        .bind(id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Assignments + user access ───────────────────────────────────────────────

pub async fn list_assignments(
    pg: &PgPool,
    server_id: &str,
) -> Result<Vec<(String, Option<Vec<String>>)>, sqlx::Error> {
    let rows: Vec<(String, Option<Vec<String>>)> = sqlx::query_as(
        "select agent_model, tools from mcp_server_agents where server_id::text = $1 order by agent_model",
    )
    .bind(server_id)
    .fetch_all(pg)
    .await?;
    Ok(rows)
}

pub async fn set_assignment(
    pg: &PgPool,
    server_id: &str,
    agent_model: &str,
    tools: Option<&[String]>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into mcp_server_agents (server_id, agent_model, tools) values ($1::uuid, $2, $3) \
         on conflict (server_id, agent_model) do update set tools = $3",
    )
    .bind(server_id)
    .bind(agent_model)
    .bind(tools)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn remove_assignment(
    pg: &PgPool,
    server_id: &str,
    agent_model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from mcp_server_agents where server_id::text = $1 and agent_model = $2")
        .bind(server_id)
        .bind(agent_model)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn list_user_access(
    pg: &PgPool,
    server_id: &str,
) -> Result<Vec<(String, bool, Option<Vec<String>>)>, sqlx::Error> {
    let rows: Vec<(String, bool, Option<Vec<String>>)> = sqlx::query_as(
        "select user_id::text, allowed, tools from mcp_user_access where server_id::text = $1",
    )
    .bind(server_id)
    .fetch_all(pg)
    .await?;
    Ok(rows)
}

pub async fn set_user_access(
    pg: &PgPool,
    server_id: &str,
    user_id: &str,
    allowed: Option<bool>,
    tools: Option<&[String]>,
) -> Result<(), sqlx::Error> {
    if allowed.is_none() {
        sqlx::query(
            "delete from mcp_user_access where server_id::text = $1 and user_id::text = $2",
        )
        .bind(server_id)
        .bind(user_id)
        .execute(pg)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "insert into mcp_user_access (server_id, user_id, allowed, tools) values ($1::uuid, $2::uuid, $3, $4) \
         on conflict (server_id, user_id) do update set allowed = $3, tools = $4",
    )
    .bind(server_id)
    .bind(user_id)
    .bind(allowed)
    .bind(tools)
    .execute(pg)
    .await?;
    Ok(())
}

// ── Per-user connected accounts ─────────────────────────────────────────────

pub async fn set_user_credentials(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    user_id: &str,
    headers: Option<&Map<String, Value>>,
) -> Result<(), String> {
    if headers.is_none() || headers.is_some_and(|h| h.is_empty()) {
        sqlx::query(
            "delete from mcp_user_credentials where server_id::text = $1 and user_id::text = $2",
        )
        .bind(server_id)
        .bind(user_id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let sealed = sb
        .seal(&Value::Object(headers.cloned().expect("checked non-empty")).to_string())
        .map_err(|e| format!("credential seal failed: {e}"))?;
    sqlx::query(
        "insert into mcp_user_credentials (server_id, user_id, headers_enc) values ($1::uuid, $2::uuid, $3) \
         on conflict (server_id, user_id) do update set headers_enc = $3, updated_at = now()",
    )
    .bind(server_id)
    .bind(user_id)
    .bind(&sealed)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn has_user_credentials(
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

pub async fn get_user_credentials(
    pg: &PgPool,
    sb: &SecretBox,
    server_id: &str,
    user_id: &str,
) -> Result<Option<Map<String, Value>>, String> {
    let row: Option<(String,)> = sqlx::query_as(
        "select headers_enc from mcp_user_credentials where server_id::text = $1 and user_id::text = $2",
    )
    .bind(server_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some((enc,)) = row else {
        return Ok(None);
    };
    let opened = sb.open(&enc).map_err(|e| e.to_string())?;
    // A failed parse reads as "no connected account" (null), which the
    // caller treats as no access — never a thrown 500.
    Ok(serde_json::from_str(&opened).ok())
}

// ── Effective resolution (the gateway's brain) ──────────────────────────────

/// The human an AGENT CALLER may act for — its owner when it is a personal
/// assistant, None otherwise. A legacy caller
/// gets None: identified, but not proven to BE that assistant — and this
/// resolution hands out that human's connected account.
async fn assistant_owner_for(
    pg: &PgPool,
    subject: &AgentSubject,
) -> Result<Option<String>, sqlx::Error> {
    if !subject_proven(subject) {
        return Ok(None);
    }
    Ok(personal_assistant_owners(pg)
        .await?
        .get(subject_model(subject))
        .cloned())
}

pub struct EffectiveMcp {
    pub server: McpServer,
    /// None = all tools; otherwise the enforced allowlist.
    pub tools: Option<Vec<String>>,
    /// Headers to speak upstream with (org's, or the acting user's sealed
    /// set) — ordered pairs, JS object order.
    pub upstream_headers: Vec<(String, String)>,
}

fn intersect(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Option<Vec<String>> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let bs: HashSet<String> = b.into_iter().collect();
            Some(a.into_iter().filter(|t| bs.contains(t)).collect())
        }
    }
}

/// What one AGENT may do on one server: assignment ∩ (for a personal
/// assistant) its owner's user access — with the owner's credentials on
/// per-user servers. None = no access at all.
pub async fn effective_mcp_for(
    pg: &PgPool,
    sb: &SecretBox,
    subject: &AgentSubject,
    server_name: &str,
) -> Result<Option<EffectiveMcp>, String> {
    let agent_model = subject_model(subject);
    let Some(server) = get_mcp_server(pg, server_name)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if !server.enabled {
        return Ok(None);
    }

    let rows: Vec<(Option<Vec<String>>,)> = sqlx::query_as(
        "select tools from mcp_server_agents where server_id::text = $1 and agent_model = $2",
    )
    .bind(&server.id)
    .bind(agent_model)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    // All-agents servers carry everyone; assignment rows become per-agent
    // tool OVERRIDES. Scoped servers require a row outright.
    let agent_tools = if rows.is_empty() {
        if !server.all_agents {
            return Ok(None);
        }
        None
    } else {
        rows[0].0.clone()
    };

    let owner = assistant_owner_for(pg, subject)
        .await
        .map_err(|e| e.to_string())?;
    let mut user_tools: Option<Vec<String>> = None;
    let mut upstream_headers: Vec<(String, String)> = server
        .headers
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
        .collect();
    if let Some(owner) = &owner {
        let access: Option<(bool, Option<Vec<String>>)> = sqlx::query_as(
            "select allowed, tools from mcp_user_access where server_id::text = $1 and user_id::text = $2",
        )
        .bind(&server.id)
        .bind(owner)
        .fetch_optional(pg)
        .await
        .map_err(|e| e.to_string())?;
        if access.as_ref().is_some_and(|(allowed, _)| !allowed) {
            return Ok(None);
        }
        user_tools = access.and_then(|(_, tools)| tools);
        if server.auth_mode == "per-user" {
            if server.oauth_enabled {
                let Some(bearer) =
                    crate::mcp::oauth::oauth_token_for(pg, sb, &server.id, owner).await?
                else {
                    return Ok(None); // not connected — the server doesn't exist for this assistant yet
                };
                set_header(
                    &mut upstream_headers,
                    "authorization",
                    &format!("Bearer {bearer}"),
                );
            } else {
                let Some(creds) = get_user_credentials(pg, sb, &server.id, owner).await? else {
                    return Ok(None);
                };
                for (k, v) in &creds {
                    if let Some(v) = v.as_str() {
                        set_header(&mut upstream_headers, k, v);
                    }
                }
            }
        }
    } else if server.auth_mode == "per-user" {
        // Org agents have no single acting user to be — per-user servers are
        // personal-assistant territory by definition.
        return Ok(None);
    }
    // Org-auth OAuth servers speak with the shared org connection.
    if server.auth_mode == "org" && server.oauth_enabled {
        let Some(bearer) = crate::mcp::oauth::oauth_token_for(pg, sb, &server.id, "org").await?
        else {
            return Ok(None); // nobody connected the org account yet
        };
        set_header(
            &mut upstream_headers,
            "authorization",
            &format!("Bearer {bearer}"),
        );
    }

    Ok(Some(EffectiveMcp {
        server,
        tools: intersect(agent_tools, user_tools),
        upstream_headers,
    }))
}

/// `{...a, k: v}` — an existing key keeps its position and takes the value;
/// a new one appends (the pairs-list spelling of the object-spread rule).
fn set_header(pairs: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some((_, v)) = pairs.iter_mut().find(|(k, _)| k == key) {
        *v = value.to_string();
    } else {
        pairs.push((key.to_string(), value.to_string()));
    }
}

// ── Discovery ───────────────────────────────────────────────────────────────

/// Ask the upstream for its tool catalog (initialize + tools/list) and cache
/// it. Ok(tools) is the refreshed catalog; Err is the route's 502 sentence.
pub async fn refresh_mcp_tools(
    pg: &PgPool,
    sb: &SecretBox,
    id: &str,
) -> Result<Vec<Value>, String> {
    let Some(server) = get_mcp_server(pg, id).await.map_err(|e| e.to_string())? else {
        return Err("not found".into());
    };
    if server.app_slug.is_some() {
        // App servers: the catalog comes from the compiled app module —
        // authors' TS/node code, owned by the app runtime, not something this
        // registry path can fetch.
        return Err(format!(
            "app \"{}\" publishes its catalog from the app module, which stays TS by rule 10 (docs/RUST-MIGRATION.md)",
            server.app_slug.unwrap_or_default()
        ));
    }
    // The Workbench is IN-PROCESS, and `talaria-workbench://core` is a routing
    // token rather than an endpoint — there is nothing to connect to. The
    // catalog reads the way the module exports it.
    let non_http = !server.url.to_lowercase().starts_with("http://")
        && !server.url.to_lowercase().starts_with("https://");
    if non_http {
        let tools = workbench_catalog();
        store_catalog(pg, id, &tools).await?;
        return Ok(tools.as_array().cloned().unwrap_or_default());
    }
    // The built-in toolkit runs as a child of this process, spawned
    // opportunistically (renders, comms reads). On a freshly booted instance
    // none of those has happened, so start it and wait rather than probing a
    // port nothing is listening on.
    if server.builtin && !crate::mcp::service::await_mcp_service(8_000).await {
        return Err("the Talaria toolkit service did not start; check the app logs".into());
    }
    let bearer = if server.oauth_enabled {
        crate::mcp::oauth::oauth_token_for(pg, sb, &server.id, "org").await?
    } else {
        None
    };
    let run = async {
        let init = org_call(
            &server,
            bearer.as_deref(),
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
            return Err(format!("upstream {}", init.status));
        }
        let list = org_call(
            &server,
            bearer.as_deref(),
            &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
            init.session.as_deref(),
        )
        .await?;
        let tools: Vec<Value> = list
            .json
            .as_ref()
            .and_then(|j| j.pointer("/result/tools"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|t| {
                        let mut e = Map::new();
                        e.insert("name".into(), t.get("name").cloned().unwrap_or(Value::Null));
                        if let Some(d) = t.get("description").and_then(Value::as_str) {
                            e.insert("description".into(), Value::String(utf16_slice(d, 300)));
                        }
                        Value::Object(e)
                    })
                    .collect()
            })
            .unwrap_or_default();
        store_catalog(pg, id, &Value::Array(tools.clone())).await?;
        Ok(tools)
    };
    run.await
}

async fn store_catalog(pg: &PgPool, id: &str, tools: &Value) -> Result<(), String> {
    sqlx::query(
        "update mcp_servers set tools = $2, tools_refreshed_at = now(), updated_at = now() where id::text = $1",
    )
    .bind(id)
    .bind(tools)
    .execute(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

// `servers_for_agent` legitimately answers about a THIRD PARTY — fleet-render
// and the /api/mcp admin listing ask "what should <model> carry?" with no
// caller in hand — so a bare model string stays accepted. It grants nothing
// on its own: the credential is never rendered, only a gateway URL, and the
// gateway re-derives access per request through `effective_mcp_for`.

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

// (Does this server have per-user credentials stored? — the pub
// `has_user_credentials` earlier in this file is that same read.)

/// model → owner_user_id for every PERSONAL assistant. The render passes a
/// bare model string — proven by construction there — so the owner map is
/// the whole lookup.
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

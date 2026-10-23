// The org-wide MCP registry — Composio's shape, self-hosted and tailored:
//   servers      registered once for the whole org (url + org headers)
//   assignments  which agents carry a server, optionally a TOOL SUBSET
//   user access  which users may exercise a server through agents acting for
//                them, optionally their own tool subset
//   team access  same, for a team (people via team_members, agents via
//                team_agents) — expanded at auth time, never write-time fan-out
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

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{Map, Value};
use sqlx::PgPool;

use talaria_agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model};
use talaria_gateway::provider::http;
use talaria_mcp_oauth::has_oauth_tokens;
use talaria_safe_fetch::{SafeFetch, safe_fetch};
use talaria_secretbox::SecretBox;
use talaria_users::{assistant_owner_for, personal_assistant_owners};

pub use crate::not_connected::{ConnectScope, NotConnected};

/// The protocol revision Talaria speaks at the MCP handshake — one revision
/// for both directions of the conversation (what our dispatchers answer and
/// what every client asks). The literal lives in mcp::jsonrpc (the leaf
/// every dispatcher shares); re-exported here for the importers that reach
/// it via the registry.
pub use talaria_mcp_jsonrpc::MCP_PROTOCOL_VERSION;

/// The column list every full-row read spells — the wire's field order IS
/// this order.
const ROW: &str = "id::text, name, label, description, url, headers, headers_sealed, \
                   timeout_secs::int8, enabled, \
                   all_agents, auth_mode::text, tools, \
                   (trunc(extract(epoch from tools_refreshed_at) * 1000))::bigint as tools_refreshed_ms, \
                   required_headers, (oauth is not null) as oauth_enabled, builtin, \
                   app_slug::text, created_by, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
                   package, env_enc::text, last_probe";

/// A registry row, every column. The jsonb columns ride
/// as the raw Value — pg owns their key order and the wire answer must carry
/// it byte-for-byte.
#[derive(Clone)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub label: String,
    pub description: Option<String>,
    pub url: String,
    /// LEGACY plaintext org headers — `{}` on every row written since the
    /// headers were sealed; a pre-sealing row keeps its plaintext here until
    /// `org_headers::seal_legacy_headers` reaches it. Never read directly:
    /// `org_headers::open_headers` is the one reader of both columns.
    pub headers_plain: Map<String, Value>,
    /// The org headers an admin typed (API keys, auth headers), sealed whole
    /// as one JSON object. None = no org headers.
    pub headers_sealed: Option<String>,
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
    /// Package servers (npm/pypi/oci): the declarations document — image,
    /// runtime args, declared env, pull state. Declarations only; the VALUES
    /// live sealed in `env_enc` and never answer a GET.
    pub package: Option<Value>,
    /// SecretBox-sealed env values + filled run-args (`pkg::SealedDoc`).
    pub env_enc: Option<String>,
    /// What the last discovery said: `{state, detail, at}` with state
    /// ok | auth-required | unreachable | error; None = never probed. The
    /// card reads it to say "needs an API key" instead of "Discover tools".
    pub last_probe: Option<Value>,
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
    headers_sealed: Option<String>,
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
    package: Option<Value>,
    env_enc: Option<String>,
    last_probe: Option<Value>,
}

impl McpServer {
    /// A marketplace package install (a document, not a jsonb `null` left by
    /// an older insert).
    pub fn is_package(&self) -> bool {
        self.package.as_ref().is_some_and(Value::is_object)
    }
}

fn server_of_row(r: ServerRow) -> McpServer {
    McpServer {
        id: r.id,
        name: r.name,
        label: r.label,
        description: r.description,
        url: r.url,
        headers_plain: r.headers.as_object().cloned().unwrap_or_default(),
        headers_sealed: r.headers_sealed,
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
        // Older inserts stored jsonb `null` for "no package"; read it as none.
        package: r.package.filter(Value::is_object),
        env_enc: r.env_enc,
        last_probe: r.last_probe,
    }
}

/// The row as the routes answer it — the ROW column order, timestamps as the
/// ISO strings `JSON.stringify(new Date(...))` produces. Org headers answer
/// as their KEY NAMES only (`header_keys`, from `org_headers::header_keys`):
/// a value never leaves the api, so this function is never handed one.
pub fn server_wire(s: &McpServer, header_keys: &[String]) -> Value {
    serde_json::json!({
        "id": s.id,
        "name": s.name,
        "label": s.label,
        "description": s.description,
        "url": s.url,
        "headerKeys": header_keys,
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
        // Declarations only — env_enc is never serialized to any wire.
        "package": s.package,
        "probe": s.last_probe,
    })
}

/// The discovery states a row remembers. `AuthRequired` is the one the card
/// turns into an action (set a key, or connect OAuth). `Reconnect` is the
/// health pass's: the org account WAS connected and the provider has since
/// refused to refresh it (`health::run_health_pass`), so the action is a fresh
/// sign-in, not a first one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeState {
    Ok,
    AuthRequired,
    Unreachable,
    Error,
    Reconnect,
}

impl ProbeState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeState::Ok => "ok",
            ProbeState::AuthRequired => "auth-required",
            ProbeState::Unreachable => "unreachable",
            ProbeState::Error => "error",
            ProbeState::Reconnect => "reconnect",
        }
    }
}

/// Remember what discovery just found. Best-effort: a failed write must not
/// turn a successful discovery into a failure, so the error is logged, not
/// returned.
pub async fn record_probe(pg: &PgPool, id: &str, state: ProbeState, detail: &str) {
    let probe = probe_json(state, detail, talaria_agent_auth::now_ms(), None);
    if let Err(e) = sqlx::query(
        "update mcp_servers set last_probe = $2, updated_at = now() where id::text = $1",
    )
    .bind(id)
    .bind(&probe)
    .execute(pg)
    .await
    {
        tracing::warn!("[mcp] probe record failed for {id}: {e}");
    }
}

/// The `last_probe` document: `{state, detail, at}`, plus whatever the health
/// pass adds (`staleAssignments`) appended after them. One builder, so a
/// discovery write and a health write can never disagree about the shape.
pub fn probe_json(
    state: ProbeState,
    detail: &str,
    at_ms: i64,
    extra: Option<&Map<String, Value>>,
) -> Value {
    let mut m = Map::new();
    m.insert("state".into(), Value::String(state.as_str().into()));
    m.insert("detail".into(), Value::String(utf16_slice(detail, 300)));
    m.insert("at".into(), Value::String(epoch_ms_to_iso(at_ms)));
    if let Some(extra) = extra {
        for (k, v) in extra {
            m.insert(k.clone(), v.clone());
        }
    }
    Value::Object(m)
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
        talaria_mcp_oauth::oauth_token_for(pg, sb, &server.id, "org").await?
    } else {
        None
    };
    let org_headers = crate::org_headers::open_headers(sb, &server)?;
    let init = org_call(
        &server,
        &org_headers,
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
        &org_headers,
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
    /// The upstream's WWW-Authenticate, when it answered 401/403 — the one
    /// sentence it offers about what it wants.
    www_authenticate: Option<String>,
}

/// One POST of the conversation: org headers plus session plus (on OAuth
/// servers) the org bearer, JSON or SSE-framed reply parsed, the session
/// header carried forward.
/// Rejection sentences in undici's shape, because these exact strings are
/// the surface callers see: transport failures read bare "fetch failed"
/// (the cause chain carries the detail, the message does not) and a timeout
/// expiry reads "The operation was aborted due to timeout".
pub use talaria_undici::undici_message;

/// The same shapes for the safe-fetch leg — except a blocked URL, whose
/// refusal sentence is itself the message that surfaces, so it passes
/// through verbatim.
fn undici_safe_message(e: talaria_safe_fetch::SafeError) -> String {
    match e {
        talaria_safe_fetch::SafeError::Timeout => "The operation was aborted due to timeout".into(),
        talaria_safe_fetch::SafeError::Blocked(b) => b.to_string(),
        talaria_safe_fetch::SafeError::Fetch(_) => "fetch failed".into(),
    }
}

async fn org_call(
    server: &McpServer,
    org_headers: &Map<String, Value>,
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
    pairs.extend(crate::org_headers::header_pairs(org_headers));
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
    let www_authenticate = headers
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    Ok(OrgReply {
        json: parse_mcp_response(&text),
        session,
        status,
        www_authenticate,
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
pub use talaria_mcp_jsonrpc::parse_mcp_response;

// ── The registry write half ─────────────────────────────────────────────────

/// [`ensure_builtin_mcp`]'s toolkit row. A const so the test can hold the
/// conflict arm to rewriting the url.
const BUILTIN_TOOLKIT_UPSERT: &str = "insert into mcp_servers (name, label, description, url, all_agents, builtin, created_by) \
     values ('talaria', 'Talaria toolkit', \
             'Talaria''s own tools: tickets, documents, knowledge, channels, research, media.', \
             $1, true, true, 'talaria') \
     on conflict (name) do update set builtin = true, all_agents = true, enabled = true, url = excluded.url";

/// The Talaria toolkit as a governable system row: every agent carries it,
/// and the same per-agent/per-person tool subsets apply — enforced by the
/// gateway like any other server. Identity/lifecycle stay locked.
///
/// The url is rewritten on every boot, not only on first insert: it is where
/// THIS process's toolkit child listens (`TALARIA_MCP_PORT`), and the MCP
/// gateway proxies every agent toolkit call to it. Kept from an earlier boot,
/// a seeded database, or before the port moved, it pointed the gateway at
/// another stack's toolkit, which refused this stack's agent keys (401).
pub async fn ensure_builtin_mcp(pg: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(BUILTIN_TOOLKIT_UPSERT)
        .bind(format!(
            "http://127.0.0.1:{}/mcp",
            talaria_mcp_service::mcp_port()
        ))
        .execute(pg)
        .await?;
    // The Workbench surface — in-process like app servers, but NOT all_agents:
    // access is the agent's Developer Agent switch (see WORKBENCH_SERVER).
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
        crate::workbench_tools()
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
    /// Package installs: the declarations document + the sealed credential
    /// blob (see pkg.rs).
    pub package: Option<&'a Value>,
    pub env_enc: Option<String>,
}

pub async fn create_mcp_server(
    pg: &PgPool,
    sb: &SecretBox,
    input: &NewServer<'_>,
) -> Result<McpServer, String> {
    // Sealed before the insert: the plaintext column is written `{}` and a
    // header value never touches a row in the clear.
    let headers_sealed =
        crate::org_headers::seal_headers(sb, &input.headers.cloned().unwrap_or_default())?;
    // The full InputWithVariables shape, with the nullish defaults baked in —
    // `value`/`variables` carry the registry's credential templates that
    // drive the connect forms.
    let declared: Vec<Value> = input
        .required_headers
        .as_array()
        .map(|a| {
            a.iter()
                .map(|h| {
                    serde_json::json!({
                        "name": h.get("name").cloned().unwrap_or(Value::Null),
                        "description": h.get("description").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        "isRequired": h.get("isRequired").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Bool(false)),
                        // `h.isSecret ?? false` — absent AND null both land on false.
                        "isSecret": h.get("isSecret").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Bool(false)),
                        "placeholder": h.get("placeholder").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        "default": h.get("default").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        "choices": h.get("choices").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        "value": h.get("value").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        "variables": h.get("variables").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // AssertSqlSafe: the interpolation is this crate's ROW column list.
    let sql = format!(
        "insert into mcp_servers (name, label, description, url, headers, headers_sealed, timeout_secs, auth_mode, required_headers, created_by, package, env_enc) \
         values ($1, $2, $3, $4, '{{}}'::jsonb, $5, $6, $7, $8, $9, $10, $11) returning {ROW}"
    );
    let row: Option<ServerRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(input.name)
        .bind(input.label.unwrap_or(input.name))
        .bind(input.description)
        .bind(input.url)
        .bind(&headers_sealed)
        .bind(input.timeout_secs)
        .bind(input.auth_mode)
        .bind(Value::Array(declared))
        .bind(input.created_by)
        // SQL NULL, never a jsonb `null`: a hosted server has NO package
        // document, and every `package.is_some()` reader means exactly that.
        .bind(input.package.cloned())
        .bind(&input.env_enc)
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
    /// Set these keys (a null value removes the key), keep every other
    /// stored header — the credentials editor's write, which never sees the
    /// stored values (GET answers key names only) and so can never send the
    /// whole map.
    pub merge_headers: Option<Map<String, Value>>,
}

pub async fn update_mcp_server(
    pg: &PgPool,
    sb: &SecretBox,
    id: &str,
    patch: &ServerPatch,
) -> Result<(), String> {
    let row: Option<(bool, Option<String>, String)> =
        sqlx::query_as("select builtin, app_slug::text, url from mcp_servers where id::text = $1")
            .bind(id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    let Some((builtin, app_slug, old_url)) = row else {
        return Ok(()); // updates below match zero rows — a no-op
    };
    if builtin
        && (patch.url.is_some()
            || patch.headers.is_some()
            || patch.merge_headers.is_some()
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
            || patch.merge_headers.is_some()
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
    // A URL that names a different host is a different server: nothing the
    // old one was trusted with may follow — see `url_host_changed`.
    let host_changed = patch
        .url
        .as_deref()
        .is_some_and(|new_url| url_host_changed(&old_url, new_url));
    if let Some(url) = &patch.url {
        // A repointed server's cached catalog is stale by definition.
        sqlx::query("update mcp_servers set url = $2, tools = '[]', tools_refreshed_at = null, updated_at = now() where id::text = $1")
            .bind(id).bind(url).execute(pg).await.map_err(|e| e.to_string())?;
    }
    if host_changed {
        drop_credentials_for_new_host(pg, id).await?;
    }
    // Replace and/or merge, opened and re-sealed under the row lock. A
    // changed credential invalidates nothing else — the catalog stays; the
    // next discovery re-proves it. A changed host starts from NO headers
    // unless this very patch names the new ones.
    let empty = Map::new();
    let replace = match (&patch.headers, host_changed) {
        (Some(h), _) => Some(h),
        (None, true) => Some(&empty),
        (None, false) => None,
    };
    crate::org_headers::write_headers(
        pg,
        sb,
        id,
        &crate::org_headers::HeaderWrite {
            replace,
            merge: patch.merge_headers.as_ref(),
        },
    )
    .await?;
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

/// Does `new_url` name a different upstream than `old_url`? The authority —
/// host and port, case-folded — decides; a changed path or scheme on the
/// same host is the same operator. An unparseable side counts as changed:
/// the package placeholder (`talaria-pkg://name`) or garbage on either side
/// is never "the same host" as a real endpoint. Pure.
pub fn url_host_changed(old_url: &str, new_url: &str) -> bool {
    fn authority(u: &str) -> Option<(String, Option<u16>)> {
        let parsed = url::Url::parse(u).ok()?;
        let host = parsed.host_str()?.to_ascii_lowercase();
        Some((host, parsed.port_or_known_default()))
    }
    match (authority(old_url), authority(new_url)) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

/// What a repointed server loses: every OAuth token the gateway would have
/// spoken to the new host (the org's and every person's), the in-flight
/// authorization states and reconnect flags that name the old provider, and
/// the stored OAuth discovery (so the next `ensure_oauth_config` asks the
/// NEW host what it wants rather than reusing the old answer). The org
/// headers are cleared by the caller's header write. The catalog is already
/// empty by now.
async fn drop_credentials_for_new_host(pg: &PgPool, id: &str) -> Result<(), String> {
    for sql in [
        "delete from mcp_oauth_tokens where server_id::text = $1",
        "delete from mcp_oauth_states where server_id::text = $1",
        "delete from mcp_oauth_reconnect where server_id::text = $1",
        "update mcp_servers set oauth = null, updated_at = now() where id::text = $1",
    ] {
        sqlx::query(sql)
            .bind(id)
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
    // A package server's container goes with its row — the reconcile pass
    // would sweep it anyway, but the sweep is lazy and the name is gone.
    let pkg_row: Option<(String,)> =
        sqlx::query_as("select name from mcp_servers where id::text = $1 and package is not null")
            .bind(id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    sqlx::query("delete from mcp_servers where id::text = $1") // assignments/access/credentials cascade
        .bind(id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    if let Some((name,)) = pkg_row {
        crate::pkg::stop_pkg(&name).await;
    }
    Ok(())
}

// ── Assignments + user/team access ──────────────────────────────────────────

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

pub async fn list_team_access(
    pg: &PgPool,
    server_id: &str,
) -> Result<Vec<(String, bool, Option<Vec<String>>)>, sqlx::Error> {
    let rows: Vec<(String, bool, Option<Vec<String>>)> = sqlx::query_as(
        "select team_id::text, allowed, tools from mcp_team_access where server_id::text = $1",
    )
    .bind(server_id)
    .fetch_all(pg)
    .await?;
    Ok(rows)
}

pub async fn set_team_access(
    pg: &PgPool,
    server_id: &str,
    team_id: &str,
    allowed: Option<bool>,
    tools: Option<&[String]>,
) -> Result<(), sqlx::Error> {
    if allowed.is_none() {
        sqlx::query(
            "delete from mcp_team_access where server_id::text = $1 and team_id::text = $2",
        )
        .bind(server_id)
        .bind(team_id)
        .execute(pg)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "insert into mcp_team_access (server_id, team_id, allowed, tools) values ($1::uuid, $2::uuid, $3, $4) \
         on conflict (server_id, team_id) do update set allowed = $3, tools = $4",
    )
    .bind(server_id)
    .bind(team_id)
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

/// The Workbench's registry name. Its access is the agent's Developer Agent
/// switch (agent_defs.developer), never an assignment or team row.
pub const WORKBENCH_SERVER: &str = "workbench";

/// Is this agent (by model) a Developer Agent?
async fn agent_is_developer(pg: &PgPool, agent_model: &str) -> Result<bool, sqlx::Error> {
    let on: Option<bool> =
        sqlx::query_scalar("select developer from agent_defs where model = $1 and enabled")
            .bind(agent_model)
            .fetch_optional(pg)
            .await?;
    Ok(on.unwrap_or(false))
}

/// Union of tool allowlists. `None` = all tools, so it dominates.
fn union_tools(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Option<Vec<String>> {
    match (a, b) {
        (None, _) | (_, None) => None,
        (Some(mut a), Some(b)) => {
            let mut seen: HashSet<String> = a.iter().cloned().collect();
            for t in b {
                if seen.insert(t.clone()) {
                    a.push(t);
                }
            }
            Some(a)
        }
    }
}

fn union_tool_lists(lists: impl IntoIterator<Item = Option<Vec<String>>>) -> Option<Vec<String>> {
    let mut iter = lists.into_iter();
    match iter.next() {
        None => None,
        Some(first) => iter.fold(first, union_tools),
    }
}

/// The gateway's verdict for one agent on one server — three answers, not
/// two, because "you may not" and "nobody connected it yet" call for
/// different replies: a refusal the agent should accept, and a sentence it
/// should repeat to its person (see `not_connected`).
pub enum McpAccess {
    /// Speak upstream with these headers, inside this allowance.
    Allowed(EffectiveMcp),
    /// No grant, an explicit deny, a disabled or unknown server.
    Denied,
    /// Granted, but the account it would speak as is not connected.
    NotConnected(NotConnected),
}

/// What one AGENT may do on one server: assignment ∪ team-agent grants,
/// intersected (for a personal assistant) with its owner's user/team access —
/// with the owner's credentials on per-user servers. Access is decided
/// BEFORE credentials, so a server the agent may not use answers `Denied`
/// whether or not anyone connected it, and only a granted one can answer
/// `NotConnected`.
pub async fn effective_mcp_for(
    pg: &PgPool,
    sb: &SecretBox,
    subject: &AgentSubject,
    server_name: &str,
) -> Result<McpAccess, String> {
    let agent_model = subject_model(subject);
    let Some(server) = get_mcp_server(pg, server_name)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(McpAccess::Denied);
    };
    if !server.enabled {
        return Ok(McpAccess::Denied);
    }

    // The Workbench is granted by the agent's Developer Agent switch and by
    // nothing else. Assignment and team rows for it are ignored, so the
    // switch and the tools can never disagree.
    let agent_tools = if server.name == WORKBENCH_SERVER {
        // A legacy shared-key caller is identified, not proven: it never
        // drives the Workbench as the agent it names.
        if !talaria_agent_auth::subject_proven(subject)
            || !agent_is_developer(pg, agent_model)
                .await
                .map_err(|e| e.to_string())?
        {
            return Ok(McpAccess::Denied);
        }
        None
    } else {
        let rows: Vec<(Option<Vec<String>>,)> = sqlx::query_as(
            "select tools from mcp_server_agents where server_id::text = $1 and agent_model = $2",
        )
        .bind(&server.id)
        .bind(agent_model)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
        let team_agent_access: Vec<(bool, Option<Vec<String>>)> = sqlx::query_as(
            "select a.allowed, a.tools \
             from mcp_team_access a \
             join team_agents t on t.team_id = a.team_id \
             where a.server_id::text = $1 and t.agent_model = $2",
        )
        .bind(&server.id)
        .bind(agent_model)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
        let team_allow_lists: Vec<Option<Vec<String>>> = team_agent_access
            .into_iter()
            .filter(|(allowed, _)| *allowed)
            .map(|(_, tools)| tools)
            .collect();
        // All-agents servers carry everyone; assignment rows become per-agent
        // tool OVERRIDES. Scoped servers require a row outright, or an allowed
        // team grant for a team the agent is on.
        if rows.is_empty() {
            if server.all_agents {
                None
            } else if team_allow_lists.is_empty() {
                return Ok(McpAccess::Denied);
            } else {
                union_tool_lists(team_allow_lists)
            }
        } else {
            union_tool_lists(std::iter::once(rows[0].0.clone()).chain(team_allow_lists))
        }
    };

    let owner = assistant_owner_for(pg, subject)
        .await
        .map_err(|e| e.to_string())?;
    let mut user_tools: Option<Vec<String>> = None;
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
            return Ok(McpAccess::Denied);
        }
        let team_rows: Vec<(bool, Option<Vec<String>>)> = sqlx::query_as(
            "select a.allowed, a.tools \
             from mcp_team_access a \
             join team_members m on m.team_id = a.team_id \
             where a.server_id::text = $1 and m.user_id::text = $2",
        )
        .bind(&server.id)
        .bind(owner)
        .fetch_all(pg)
        .await
        .map_err(|e| e.to_string())?;
        let any_team_deny = team_rows.iter().any(|(allowed, _)| !*allowed);
        let any_team_allow = team_rows.iter().any(|(allowed, _)| *allowed);
        let user_allow = access.as_ref().is_some_and(|(allowed, _)| *allowed);
        if any_team_deny && !any_team_allow && !user_allow {
            return Ok(McpAccess::Denied);
        }
        let mut allowed_lists = Vec::new();
        if user_allow {
            allowed_lists.push(access.and_then(|(_, tools)| tools));
        }
        for (allowed, tools) in team_rows {
            if allowed {
                allowed_lists.push(tools);
            }
        }
        user_tools = union_tool_lists(allowed_lists);
    } else if server.auth_mode == "per-user" {
        // Org agents have no single acting user to be — per-user servers are
        // personal-assistant territory by definition. Granted, so it is not a
        // refusal; but nothing anyone connects will change it.
        return Ok(McpAccess::NotConnected(NotConnected {
            scope: ConnectScope::PerUserOnOrgAgent,
            server,
            tools: agent_tools,
        }));
    }
    let tools = intersect(agent_tools, user_tools);

    // The account the call speaks as: the owner's on per-user servers, the
    // org's on org-auth OAuth servers. Missing → NotConnected, carrying the
    // allowance so the in-house tools/list shows what the agent will get.
    let mut speak_as: Vec<(String, String)> = Vec::new();
    if let Some(owner) = &owner
        && server.auth_mode == "per-user"
    {
        if server.oauth_enabled {
            let Some(bearer) =
                talaria_mcp_oauth::oauth_token_for(pg, sb, &server.id, owner).await?
            else {
                return Ok(McpAccess::NotConnected(NotConnected {
                    scope: ConnectScope::PerUser,
                    server,
                    tools,
                }));
            };
            speak_as.push(("authorization".into(), format!("Bearer {bearer}")));
        } else {
            let Some(creds) = get_user_credentials(pg, sb, &server.id, owner).await? else {
                return Ok(McpAccess::NotConnected(NotConnected {
                    scope: ConnectScope::PerUser,
                    server,
                    tools,
                }));
            };
            speak_as.extend(crate::org_headers::header_pairs(&creds));
        }
    }
    // Org-auth OAuth servers speak with the shared org connection — never
    // for a legacy shared-key caller, whose identity is asserted, not
    // proven (subject_proven's contract: no OAuth tokens).
    if server.auth_mode == "org" && server.oauth_enabled {
        if !talaria_agent_auth::subject_proven(subject) {
            return Ok(McpAccess::Denied);
        }
        let Some(bearer) = talaria_mcp_oauth::oauth_token_for(pg, sb, &server.id, "org").await?
        else {
            return Ok(McpAccess::NotConnected(NotConnected {
                scope: ConnectScope::Org,
                server,
                tools,
            }));
        };
        speak_as.push(("authorization".into(), format!("Bearer {bearer}")));
    }

    // The org headers (publisher-fixed ones on per-user servers too) first,
    // then the account's own over them — `{...org, ...account}`.
    let mut upstream_headers =
        crate::org_headers::header_pairs(&crate::org_headers::open_headers(sb, &server)?);
    for (k, v) in &speak_as {
        set_header(&mut upstream_headers, k, v);
    }

    Ok(McpAccess::Allowed(EffectiveMcp {
        server,
        tools,
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
    let Some(mut server) = get_mcp_server(pg, id).await.map_err(|e| e.to_string())? else {
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
    // Package servers split by transport: stdio packages converse through
    // the pump (the cached handshake answers initialize); oci-http packages
    // relay like remotes, against the container's resolved URL — shadow the
    // row and fall through to the ordinary conversation below.
    // A package row whose spec does not parse yet is one the install's
    // background pull has not pinned — it must not fall through to the
    // in-process branch below and come back wearing another server's catalog.
    if server.package.is_some()
        && crate::pkg::PkgSpec::of(server.package.as_ref().unwrap_or(&Value::Null)).is_none()
    {
        return Err("the package is still being pulled and pinned; ask again shortly".into());
    }
    if let Some(spec) = server.package.as_ref().and_then(crate::pkg::PkgSpec::of) {
        if spec.transport == "http" {
            let url = crate::pkg::ensure_http(sb, &server, &spec).await?;
            server = McpServer { url, ..server };
        } else {
            let init = crate::pkg::pkg_call(
                pg,
                sb,
                &server,
                &spec,
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
            )
            .await
            .map_err(|e| format!("package server unavailable: {e}"))?;
            if init.1.get("error").is_some() {
                return Err(format!(
                    "package server refused the handshake: {}",
                    init.1
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                ));
            }
            let list = crate::pkg::pkg_call(
                pg,
                sb,
                &server,
                &spec,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list",
                    "params": {},
                }),
            )
            .await
            .map_err(|e| format!("package server unavailable: {e}"))?;
            let tools = list
                .1
                .pointer("/result/tools")
                .cloned()
                .unwrap_or_else(|| Value::Array(Vec::new()));
            store_catalog(pg, id, &tools).await?;
            return Ok(tools.as_array().cloned().unwrap_or_default());
        }
    }
    // The Workbench is IN-PROCESS, and `talaria-workbench://core` is a routing
    // token rather than an endpoint — there is nothing to connect to. The
    // catalog reads the way the module exports it.
    let non_http = !server.url.to_lowercase().starts_with("http://")
        && !server.url.to_lowercase().starts_with("https://");
    if non_http {
        // Only the Workbench answers from the module; any other non-http URL
        // is a routing token this path does not know how to ask.
        if server.name != WORKBENCH_SERVER {
            return Err(format!(
                "\"{}\" is not an endpoint this registry can ask for a catalog",
                server.url
            ));
        }
        let tools = workbench_catalog();
        store_catalog(pg, id, &tools).await?;
        return Ok(tools.as_array().cloned().unwrap_or_default());
    }
    // The built-in toolkit runs as a child of this process, spawned
    // opportunistically (renders, comms reads). On a freshly booted instance
    // none of those has happened, so start it and wait rather than probing a
    // port nothing is listening on.
    if server.builtin && !talaria_mcp_service::await_mcp_service(8_000).await {
        return Err("the Talaria toolkit service did not start; check the app logs".into());
    }
    let bearer = if server.oauth_enabled {
        talaria_mcp_oauth::oauth_token_for(pg, sb, &server.id, "org").await?
    } else {
        None
    };
    // An org-auth OAuth server nobody has connected yet cannot be asked
    // anything: say so instead of relaying the 401 it would answer. A grant
    // the provider refused to refresh — under this very read, under the
    // gateway's, or an earlier pass's — was dropped AND flagged at the drop
    // (`oauth_token_for`), so the flag is what makes it a reconnect rather
    // than a first connect.
    if server.oauth_enabled && server.auth_mode == "org" && bearer.is_none() && !server.builtin {
        let gone = !has_oauth_tokens(pg, &server.id, "org")
            .await
            .unwrap_or(false);
        if !gone {
            // The grant is still there: the token endpoint did not answer
            // the refresh, which says nothing about the account.
            let msg = "the provider's token endpoint did not answer the refresh of the org \
                       account; check again shortly";
            record_probe(pg, id, ProbeState::Error, msg).await;
            return Err(msg.into());
        }
        if crate::health::reconnect_flagged(pg, id, "org").await {
            let msg = crate::health::REFRESH_REJECTED;
            record_probe(pg, id, ProbeState::Reconnect, msg).await;
            return Err(msg.into());
        }
        let msg = "this server authenticates with OAuth and the org account is not connected yet \
                   — Connect on the server card first";
        record_probe(pg, id, ProbeState::AuthRequired, msg).await;
        return Err(msg.into());
    }
    let org_headers = crate::org_headers::open_headers(sb, &server)?;
    match converse_catalog(&server, &org_headers, bearer.as_deref()).await {
        Ok(tools) => {
            store_catalog(pg, id, &Value::Array(tools.clone())).await?;
            record_probe(pg, id, ProbeState::Ok, &format!("{} tool(s)", tools.len())).await;
            Ok(tools)
        }
        Err(CatalogFailure::Unreachable(e)) => {
            record_probe(pg, id, ProbeState::Unreachable, &e).await;
            Err(e)
        }
        Err(CatalogFailure::Auth {
            status,
            www_authenticate,
        }) => Err(auth_required(pg, &server, status, www_authenticate.as_deref()).await),
        Err(CatalogFailure::Status(status)) => {
            let msg = format!("upstream {status}");
            record_probe(pg, id, ProbeState::Error, &msg).await;
            Err(msg)
        }
        // A tools/list that never answered after a good handshake was never
        // recorded — the handshake already proved the server is there.
        Err(CatalogFailure::List(e)) => Err(e),
    }
}

/// Why a catalog conversation produced no catalog — the outcomes before
/// anything is recorded, so discovery (a person pressed a button) and the
/// health pass (the schedule did) each decide what to write.
pub(crate) enum CatalogFailure {
    /// `initialize` never got an answer: DNS, refused, TLS, timeout.
    Unreachable(String),
    /// `initialize` answered 401/403.
    Auth {
        status: u16,
        www_authenticate: Option<String>,
    },
    /// `initialize` answered some other 4xx/5xx.
    Status(u16),
    /// `initialize` answered and `tools/list` did not.
    List(String),
}

/// THE CATALOG CONVERSATION — `initialize` then `tools/list`, as the org,
/// over the same `org_call` every platform conversation uses. Shared by
/// discovery and the health pass so the two can never ask a server
/// differently; writes nothing.
pub(crate) async fn converse_catalog(
    server: &McpServer,
    org_headers: &Map<String, Value>,
    bearer: Option<&str>,
) -> Result<Vec<Value>, CatalogFailure> {
    let init = initialize(server, org_headers, bearer)
        .await
        .map_err(CatalogFailure::Unreachable)?;
    if init.status == 401 || init.status == 403 {
        return Err(CatalogFailure::Auth {
            status: init.status,
            www_authenticate: init.www_authenticate,
        });
    }
    if init.status >= 400 {
        return Err(CatalogFailure::Status(init.status));
    }
    let list = org_call(
        server,
        org_headers,
        bearer,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        init.session.as_deref(),
    )
    .await
    .map_err(CatalogFailure::List)?;
    Ok(list
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
                        e.insert("description".into(), Value::String(d.to_string()));
                    }
                    Value::Object(e)
                })
                .collect()
        })
        .map(|tools| bound_catalog(&Value::Array(tools)))
        .unwrap_or_default())
}

/// The most a cached tool name may run to, in UTF-16 units (what the UI
/// measures in); an entry whose name is empty, missing, not a string or
/// over this is not a tool this registry will offer.
pub(crate) const TOOL_NAME_MAX: usize = 200;
/// The most of a tool's description the cache keeps.
pub(crate) const TOOL_DESCRIPTION_MAX: usize = 300;
/// The most of a tool's `inputSchema` the cache keeps, in serialized bytes;
/// a larger one is replaced by the permissive object schema plus
/// `inputSchemaTruncated: true`, so the catalog stays a list of valid tools
/// without carrying a document an upstream chose the size of.
pub(crate) const TOOL_SCHEMA_MAX_BYTES: usize = 32 * 1024;

/// THE CATALOG BOUND — every tool list an upstream answered, whichever path
/// asked (hosted, package, Workbench), passes through here before it is
/// stored: a name over `TOOL_NAME_MAX` or empty drops the entry, a
/// description is clipped to `TOOL_DESCRIPTION_MAX`, an `inputSchema` over
/// `TOOL_SCHEMA_MAX_BYTES` becomes the marker. Every other field an entry
/// carries is kept as it came. Anything that is not an array of objects
/// bounds to an empty catalog. Pure, pinned by tests.
pub(crate) fn bound_catalog(tools: &Value) -> Vec<Value> {
    let Some(entries) = tools.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|t| {
            let obj = t.as_object()?;
            let name = obj.get("name").and_then(Value::as_str)?;
            if name.is_empty() || name.encode_utf16().count() > TOOL_NAME_MAX {
                return None;
            }
            let mut e = obj.clone();
            if let Some(d) = obj.get("description").and_then(Value::as_str) {
                e.insert(
                    "description".into(),
                    Value::String(utf16_slice(d, TOOL_DESCRIPTION_MAX)),
                );
            }
            if let Some(schema) = obj.get("inputSchema")
                && schema.to_string().len() > TOOL_SCHEMA_MAX_BYTES
            {
                e.insert(
                    "inputSchema".into(),
                    serde_json::json!({ "type": "object" }),
                );
                e.insert("inputSchemaTruncated".into(), Value::Bool(true));
            }
            Some(Value::Object(e))
        })
        .collect()
}

/// The handshake alone — `(status, WWW-Authenticate)` or the transport
/// sentence. The health pass's reachability probe for per-user servers,
/// where a 401 is the expected answer and proves the host is there.
pub(crate) async fn initialize_status(
    server: &McpServer,
    org_headers: &Map<String, Value>,
) -> Result<u16, String> {
    initialize(server, org_headers, None)
        .await
        .map(|r| r.status)
}

async fn initialize(
    server: &McpServer,
    org_headers: &Map<String, Value>,
    bearer: Option<&str>,
) -> Result<OrgReply, String> {
    org_call(
        server,
        org_headers,
        bearer,
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
    .await
}

/// A 401/403 from discovery, turned into the one sentence that says what to
/// do. The server's own challenge header is quoted when it sent one. On a
/// server not yet known to speak OAuth, this is also the moment to find out:
/// a server registered while it was unreachable, or one that only started
/// advertising OAuth later, gets its config discovered here and the sentence
/// points at Connect instead of at a key field.
async fn auth_required(
    pg: &PgPool,
    server: &McpServer,
    status: u16,
    www_authenticate: Option<&str>,
) -> String {
    let msg = auth_required_message(pg, server, status, www_authenticate).await;
    record_probe(pg, &server.id, ProbeState::AuthRequired, &msg).await;
    msg
}

/// The sentence alone, unrecorded — the health pass writes its probe in one
/// atomic swap of its own (`health::swap_probe`).
pub(crate) async fn auth_required_message(
    pg: &PgPool,
    server: &McpServer,
    status: u16,
    www_authenticate: Option<&str>,
) -> String {
    let challenge = www_authenticate
        .map(|w| utf16_slice(w.trim(), 200))
        .filter(|w| !w.is_empty());
    let quoted = challenge
        .as_ref()
        .map(|c| format!(" (it said: {c})"))
        .unwrap_or_default();
    let discovered_oauth = !server.oauth_enabled
        && !server.builtin
        && server.package.is_none()
        && matches!(
            talaria_mcp_oauth::ensure_oauth_config(pg, &server.id, &server.url).await,
            Ok(Some(_))
        );
    if discovered_oauth || server.oauth_enabled {
        if server.auth_mode == "per-user" {
            format!(
                "the server requires authentication ({status}) and speaks OAuth: each person connects \
                 their own account under Settings → Connections{quoted}"
            )
        } else {
            format!(
                "the server requires authentication ({status}) and speaks OAuth: Connect the org \
                 account on the server card{quoted}"
            )
        }
    } else if server.auth_mode == "per-user" {
        format!(
            "the server requires authentication ({status}): each person connects their own \
             credentials under Settings → Connections{quoted}"
        )
    } else {
        format!(
            "the server requires authentication ({status}): set its API key or auth header under \
             Credentials on the server card{quoted}"
        )
    }
}

/// Write a server's catalog — bounded first (`bound_catalog`), whatever
/// path hands it in.
pub(crate) async fn store_catalog(pg: &PgPool, id: &str, tools: &Value) -> Result<(), String> {
    let tools = Value::Array(bound_catalog(tools));
    sqlx::query(
        "update mcp_servers set tools = $2, tools_refreshed_at = now(), updated_at = now() where id::text = $1",
    )
    .bind(id)
    .bind(&tools)
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

// (Does this server have per-user credentials stored? — the pub
// `has_user_credentials` earlier in this file is that same read.)

#[derive(Debug, Clone)]
pub struct AgentServer {
    pub name: String,
    pub timeout_secs: Option<i64>,
    /// Carried, but the account it speaks as is not connected yet — the
    /// gateway answers its handshake in-house and every call with the
    /// connect sentence (`not_connected`).
    pub needs_connection: bool,
}

/// Every server an agent should carry in its rendered config (gateway URLs).
///
/// A server NOBODY CONNECTED YET is carried too. The gateway answers its
/// initialize in-house, so the harness keeps it, and a call answers with the
/// sentence that says who connects it and where — the agent can relay that,
/// where a missing server could only ever be silence. What stays out is what
/// the gateway would REFUSE (a denied owner, no grant), and a per-user server
/// on an org agent unless someone granted it to that agent by name: an
/// all-agents per-user server is meant for personal assistants, and listing
/// it on every org agent would only advertise tools none of them can use.
pub async fn servers_for_agent(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Vec<AgentServer>, sqlx::Error> {
    // (name, timeout_secs, auth_mode, oauth_enabled, id, explicitly granted)
    let rows: Vec<(String, Option<i64>, String, bool, String, bool)> = sqlx::query_as(
        "select s.name, s.timeout_secs::int8, s.auth_mode, (s.oauth is not null), s.id::text, \
           (exists ( \
             select 1 from mcp_server_agents a where a.server_id = s.id and a.agent_model = $1 \
           ) or exists ( \
             select 1 from mcp_team_access t \
             join team_agents ta on ta.team_id = t.team_id \
             where t.server_id = s.id and ta.agent_model = $1 and t.allowed \
           )) as explicit \
         from mcp_servers s \
         where s.enabled and not s.builtin and case when s.name = $2 then exists ( \
           select 1 from agent_defs d where d.model = $1 and d.enabled and d.developer \
         ) else (s.all_agents or exists ( \
           select 1 from mcp_server_agents a where a.server_id = s.id and a.agent_model = $1 \
         ) or exists ( \
           select 1 from mcp_team_access t \
           join team_agents ta on ta.team_id = t.team_id \
           where t.server_id = s.id and ta.agent_model = $1 and t.allowed \
         )) end \
         order by s.name",
    )
    .bind(agent_model)
    .bind(WORKBENCH_SERVER)
    .fetch_all(pg)
    .await?;
    let owner = personal_assistant_owners(pg)
        .await?
        .get(agent_model)
        .cloned();
    let mut out = Vec::new();
    for (name, timeout_secs, auth_mode, oauth_enabled, id, explicit) in rows {
        let per_user = auth_mode == "per-user";
        let needs_connection = match &owner {
            None if per_user => {
                if !explicit {
                    continue;
                }
                true // nobody to be — the gateway says so on every call
            }
            None => oauth_enabled && !has_oauth_tokens(pg, &id, "org").await?,
            Some(owner) => {
                // A PA skips servers its owner is denied — by a person rule,
                // or by a team rule with no allow beside it: the same verdict
                // `effective_mcp_for` reaches at call time, BEFORE it looks
                // at credentials, so a config never lists a server the
                // gateway then refuses.
                if owner_denied(pg, &id, owner).await? {
                    continue;
                }
                if per_user {
                    let connected = if oauth_enabled {
                        has_oauth_tokens(pg, &id, owner).await?
                    } else {
                        has_user_credentials(pg, &id, owner).await?
                    };
                    !connected
                } else {
                    oauth_enabled && !has_oauth_tokens(pg, &id, "org").await?
                }
            }
        };
        out.push(AgentServer {
            name,
            timeout_secs,
            needs_connection,
        });
    }
    Ok(out)
}

/// The owner-side deny `effective_mcp_for` applies: an explicit person deny,
/// or a team deny with no allow (person or team) beside it.
pub async fn owner_denied(pg: &PgPool, server_id: &str, owner: &str) -> Result<bool, sqlx::Error> {
    let access: Option<(bool,)> = sqlx::query_as(
        "select allowed from mcp_user_access where server_id::text = $1 and user_id::text = $2",
    )
    .bind(server_id)
    .bind(owner)
    .fetch_optional(pg)
    .await?;
    if access.is_some_and(|(allowed,)| !allowed) {
        return Ok(true);
    }
    let team_rows: Vec<(bool,)> = sqlx::query_as(
        "select a.allowed from mcp_team_access a \
         join team_members m on m.team_id = a.team_id \
         where a.server_id::text = $1 and m.user_id::text = $2",
    )
    .bind(server_id)
    .bind(owner)
    .fetch_all(pg)
    .await?;
    let any_deny = team_rows.iter().any(|(a,)| !*a);
    let any_allow = team_rows.iter().any(|(a,)| *a);
    let user_allow = access.is_some_and(|(allowed,)| allowed);
    Ok(any_deny && !any_allow && !user_allow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The builtin toolkit row's url is this process's toolkit port, so a
    /// re-boot must rewrite it: the conflict arm of the upsert carries it.
    /// (It was insert-only, and a seeded worktree database kept main's port.)
    #[test]
    fn the_builtin_toolkit_upsert_rewrites_the_url_on_every_boot() {
        let (_, conflict) = BUILTIN_TOOLKIT_UPSERT
            .split_once("on conflict (name) do update set")
            .expect("the upsert has a conflict arm");
        assert!(conflict.contains("url = excluded.url"), "{conflict}");
        // and still re-asserts what makes it the builtin
        for keep in ["builtin = true", "all_agents = true", "enabled = true"] {
            assert!(conflict.contains(keep), "{keep}");
        }
    }

    /// The repoint rule: a different host or port is a different server and
    /// loses every credential; a different path, scheme or case on the same
    /// authority is the same one. The DB side (`drop_credentials_for_new_host`
    /// and the header clear in `update_mcp_server`) hangs off this answer.
    #[test]
    fn a_url_change_is_a_host_change_only_when_the_authority_moves() {
        // Same operator: path, scheme, case, explicit default port.
        assert!(!url_host_changed(
            "https://mcp.linear.app/mcp",
            "https://mcp.linear.app/sse"
        ));
        assert!(!url_host_changed(
            "https://mcp.linear.app/mcp",
            "https://MCP.Linear.app:443/mcp"
        ));
        assert!(!url_host_changed(
            "http://localhost:8080/mcp",
            "http://localhost:8080/v2"
        ));
        // Moved: host, subdomain, port, scheme-implied port.
        assert!(url_host_changed(
            "https://mcp.linear.app/mcp",
            "https://alice.example/mcp"
        ));
        assert!(url_host_changed(
            "https://mcp.linear.app/mcp",
            "https://evil.mcp.linear.app/mcp"
        ));
        assert!(url_host_changed(
            "http://localhost:8080/mcp",
            "http://localhost:8081/mcp"
        ));
        assert!(url_host_changed(
            "https://mcp.linear.app/mcp",
            "http://mcp.linear.app/mcp"
        ));
        // Unparseable on either side is never "the same host".
        assert!(url_host_changed("talaria-pkg://x", "https://a.example/mcp"));
        assert!(url_host_changed("https://a.example/mcp", "not a url"));
        assert!(url_host_changed("", ""));
    }

    /// The catalog bound, on the shapes upstreams answer with: a name over
    /// the cap or missing drops the entry, a long description is clipped
    /// in UTF-16 units, a huge schema becomes the marker, and every other
    /// field rides through untouched.
    #[test]
    fn the_catalog_bound_drops_caps_and_marks_but_keeps_the_rest() {
        let long_name = "n".repeat(TOOL_NAME_MAX + 1);
        let edge_name = "n".repeat(TOOL_NAME_MAX);
        let long_desc = "d".repeat(TOOL_DESCRIPTION_MAX + 50);
        let big_schema = json!({ "type": "object", "properties": {
            "blob": { "type": "string", "description": "x".repeat(TOOL_SCHEMA_MAX_BYTES) } } });
        let small_schema = json!({ "type": "object", "properties": { "q": { "type": "string" } } });
        let tools = json!([
            { "name": "search", "description": long_desc, "inputSchema": small_schema,
              "annotations": { "readOnlyHint": true } },
            { "name": "", "description": "empty name" },
            { "name": long_name, "description": "too long" },
            { "name": 42 },
            { "description": "no name" },
            "not an object",
            { "name": edge_name },
            { "name": "dump", "inputSchema": big_schema },
            { "name": "emoji", "description": "😀".repeat(200) },
        ]);
        let out = bound_catalog(&tools);
        assert_eq!(out.len(), 4, "{out:?}");
        assert_eq!(out[0]["name"], "search");
        assert_eq!(
            out[0]["description"]
                .as_str()
                .unwrap()
                .encode_utf16()
                .count(),
            TOOL_DESCRIPTION_MAX
        );
        assert_eq!(out[0]["inputSchema"], small_schema);
        assert_eq!(out[0]["annotations"]["readOnlyHint"], true);
        assert!(out[0].get("inputSchemaTruncated").is_none());
        assert_eq!(out[1]["name"].as_str().unwrap().len(), TOOL_NAME_MAX);
        assert_eq!(out[2]["name"], "dump");
        assert_eq!(out[2]["inputSchema"], json!({ "type": "object" }));
        assert_eq!(out[2]["inputSchemaTruncated"], true);
        // A 200-emoji description is 400 UTF-16 units: clipped to 150 emoji.
        assert_eq!(out[3]["description"].as_str().unwrap().chars().count(), 150);
        // Not a list at all: nothing, never a panic.
        assert!(bound_catalog(&json!(null)).is_empty());
        assert!(bound_catalog(&json!({ "name": "x" })).is_empty());
    }

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

    #[test]
    fn server_wire_carries_header_keys_never_values() {
        let kek = talaria_secretbox::derive_kek("server-wire-test");
        let dek = talaria_secretbox::new_dek().expect("entropy");
        let sb =
            SecretBox::from_parts(kek, std::collections::HashMap::from([(1u32, dek)]), Some(1));
        let mut sealed_map = Map::new();
        sealed_map.insert("X-Api-Key".into(), json!("sk-sealed-value-1"));
        let sealed = crate::org_headers::seal_headers(&sb, &sealed_map)
            .unwrap()
            .expect("non-empty seals");
        let mut legacy = Map::new();
        legacy.insert("Authorization".into(), json!("Bearer legacy-plain-2"));
        let server = McpServer {
            id: "00000000-0000-0000-0000-000000000003".into(),
            name: "exa".into(),
            label: "Exa".into(),
            description: None,
            url: "https://mcp.exa.ai/mcp".into(),
            headers_plain: legacy,
            headers_sealed: Some(sealed.clone()),
            timeout_secs: None,
            enabled: true,
            all_agents: false,
            auth_mode: "org".into(),
            tools: json!([]),
            tools_refreshed_ms: None,
            required_headers: json!([]),
            oauth_enabled: false,
            builtin: false,
            app_slug: None,
            created_by: None,
            created_ms: 0,
            package: None,
            env_enc: None,
            last_probe: None,
        };
        let mut keys = crate::org_headers::header_keys(&sb, &server).unwrap();
        keys.sort();
        let wire = server_wire(&server, &keys);
        let text = wire.to_string();
        for leak in ["sk-sealed-value-1", "legacy-plain-2", sealed.as_str()] {
            assert!(
                !text.contains(leak),
                "the wire must not carry {leak}: {text}"
            );
        }
        assert_eq!(wire["headerKeys"], json!(["Authorization", "X-Api-Key"]));
        assert!(
            wire.get("headers").is_none(),
            "no value-bearing headers member at all"
        );
    }

    #[test]
    fn union_tools_none_means_all() {
        assert_eq!(union_tools(None, Some(vec!["a".into()])), None);
        assert_eq!(union_tools(Some(vec!["a".into()]), None), None);
        assert_eq!(
            union_tools(Some(vec!["a".into()]), Some(vec!["b".into(), "a".into()])),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(union_tool_lists(Vec::<Option<Vec<String>>>::new()), None);
        assert_eq!(
            union_tool_lists([Some(vec!["x".into()]), Some(vec!["y".into()])]),
            Some(vec!["x".into(), "y".into()])
        );
    }
}

// A SERVER NOBODY CONNECTED YET — answered at the gateway, in plain words.
//
// "No access" and "not connected" used to be the same 403, so an agent
// asked to use Notion could only report a refusal, never what would fix
// it. They are different facts: a refusal is policy, and the agent should
// stop; a missing connection is one click away, and the agent should say
// whose click and where. `effective_mcp_for` now tells them apart, and this
// module is what the gateway answers with for the second:
//
//   initialize  a valid handshake in-house, so the harness KEEPS the server
//               (a client whose initialize fails drops the server, and every
//               tool on it, for the session's lifetime — and the agent can
//               never relay what it cannot reach);
//   tools/list  the cached catalog, filtered to the caller's allowance, so
//               the agent still knows what the server would offer;
//   anything    a JSON-RPC error, code -32001, whose message is the one
//   else        sentence the model should repeat to its person.
//
// Pure functions over the resolved row: the route supplies the instance's
// base URL (it owns the request) and frames the HTTP answer.

use serde_json::{Map, Value, json};
use talaria_mcp_jsonrpc::{negotiated_revision, result, rpc_error};

use crate::registry::McpServer;

/// The JSON-RPC error code for "this server is not connected". In the
/// implementation-defined server-error range (-32000..-32099), next to the
/// gateway's other refusals.
pub const NOT_CONNECTED_CODE: i64 = -32001;

/// The route a person connects their own accounts on (Settings → Connections).
pub const CONNECTIONS_PATH: &str = "/settings/connections";
/// The route an admin connects the org account on (MCP → External).
pub const MCP_ADMIN_PATH: &str = "/mcp/external";

/// The name of the stand-in tool listed when the catalog has nothing to show
/// (a per-user OAuth server has never been discovered: discovery needs the
/// token nobody has yet). Without it the agent would see no tools, never
/// call one, and never learn the sentence below.
pub const PLACEHOLDER_TOOL: &str = "connection_required";

/// Whose connection is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectScope {
    /// A per-user server on a personal assistant whose owner has not
    /// connected their own account.
    PerUser,
    /// An org-auth OAuth server whose org account nobody has connected.
    Org,
    /// A per-user server on an org agent, which has no person to act as —
    /// nothing can be connected; the server is for personal assistants.
    PerUserOnOrgAgent,
}

/// The gateway's "allowed, but not connected" verdict: the row (for its
/// name, label and cached catalog) and the tool allowance that would apply
/// once connected.
pub struct NotConnected {
    pub scope: ConnectScope,
    pub server: McpServer,
    /// None = every tool; otherwise the allowlist.
    pub tools: Option<Vec<String>>,
}

impl NotConnected {
    /// What the sentence calls the server: its label, or its name when the
    /// label is blank.
    pub fn label(&self) -> &str {
        display_label(&self.server)
    }

    pub fn server_name(&self) -> &str {
        &self.server.name
    }

    /// Where the missing connection is made, or None when nothing can be.
    pub fn connect_url(&self, base: &str) -> Option<String> {
        connect_url(self.scope, base)
    }

    /// The sentence the model reads.
    pub fn message(&self, base: &str) -> String {
        not_connected_message(self.scope, self.label(), base)
    }
}

fn display_label(server: &McpServer) -> &str {
    let label = server.label.trim();
    if label.is_empty() {
        &server.name
    } else {
        label
    }
}

/// The page the scope's connection is made on.
pub fn connect_url(scope: ConnectScope, base: &str) -> Option<String> {
    let base = base.trim_end_matches('/');
    match scope {
        ConnectScope::PerUser => Some(format!("{base}{CONNECTIONS_PATH}")),
        ConnectScope::Org => Some(format!("{base}{MCP_ADMIN_PATH}")),
        ConnectScope::PerUserOnOrgAgent => None,
    }
}

/// One plain, actionable sentence per scope — written for the person the
/// agent is talking to, because that is who the agent will repeat it to.
pub fn not_connected_message(scope: ConnectScope, label: &str, base: &str) -> String {
    let url = connect_url(scope, base).unwrap_or_default();
    match scope {
        ConnectScope::PerUser => format!(
            "Your {label} account isn't connected to Talaria. Connect it at {url}, then try again."
        ),
        ConnectScope::Org => format!(
            "The workspace's {label} connection isn't set up yet. An admin connects it on the MCP \
             page at {url}."
        ),
        ConnectScope::PerUserOnOrgAgent => {
            format!("{label} is a per-user server; only a personal assistant can use it.")
        }
    }
}

/// The tools/list entries: the cached catalog, filtered to the allowance,
/// each made a valid MCP Tool (clients validate `inputSchema` as required,
/// and the registry caches name + description only). An empty result lists
/// the placeholder, so the agent has something to call that answers with
/// the sentence.
pub fn listed_tools(catalog: &Value, allowed: Option<&[String]>, message: &str) -> Vec<Value> {
    let mut out: Vec<Value> = catalog
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t.get("name").and_then(Value::as_str)?;
            if allowed.is_some_and(|a| !a.iter().any(|x| x == name)) {
                return None;
            }
            let mut e = Map::new();
            e.insert("name".into(), Value::String(name.to_string()));
            if let Some(d) = t.get("description").and_then(Value::as_str) {
                e.insert("description".into(), Value::String(d.to_string()));
            }
            e.insert(
                "inputSchema".into(),
                t.get("inputSchema")
                    .filter(|s| s.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({ "type": "object" })),
            );
            Some(Value::Object(e))
        })
        .collect();
    if out.is_empty() {
        out.push(json!({
            "name": PLACEHOLDER_TOOL,
            "description": format!("Not usable yet. {message}"),
            "inputSchema": { "type": "object" },
        }));
    }
    out
}

/// The whole in-house answer for one request: the HTTP status and the body
/// (None = a 202 notification ack). `rpc` is the parsed body, None when it
/// did not parse.
///
/// JSON-RPC's rules hold here as they would upstream: a message without an
/// `id` is a notification and gets no reply, whatever its method; a batch
/// gets an array of the replies its requests earn (nothing at all when it
/// carried only notifications); an empty batch is one Invalid Request.
pub fn house_answer(nc: &NotConnected, rpc: Option<&Value>, base: &str) -> (u16, Option<Value>) {
    let message = nc.message(base);
    match rpc {
        Some(Value::Object(obj)) => match answer_one(nc, obj, &message) {
            Some(reply) => (200, Some(reply)),
            None => (202, None),
        },
        Some(Value::Array(batch)) if batch.is_empty() => (
            200,
            Some(rpc_error(
                &Value::Null,
                -32600,
                "Invalid Request: an empty batch",
            )),
        ),
        Some(Value::Array(batch)) => {
            let replies: Vec<Value> = batch
                .iter()
                .filter_map(|el| match el {
                    Value::Object(obj) => answer_one(nc, obj, &message),
                    // Not a request object: JSON-RPC answers it, id null.
                    _ => Some(rpc_error(&Value::Null, -32600, "Invalid Request")),
                })
                .collect();
            if replies.is_empty() {
                (202, None)
            } else {
                (200, Some(Value::Array(replies)))
            }
        }
        // Nothing parseable (or a bare scalar): one error, no id to echo —
        // still the sentence, since it is what the model needs to read.
        _ => (
            200,
            Some(rpc_error(&Value::Null, NOT_CONNECTED_CODE, &message)),
        ),
    }
}

/// One request object's reply, or None for a notification.
fn answer_one(nc: &NotConnected, obj: &Map<String, Value>, message: &str) -> Option<Value> {
    // A notification (no id) expects no reply — acknowledged, not answered.
    let id = obj.get("id")?;
    let method = obj.get("method").and_then(Value::as_str).unwrap_or("");
    Some(match method {
        "initialize" => {
            let requested = obj
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str);
            result(
                id,
                json!({
                    "protocolVersion": negotiated_revision(requested),
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": nc.server_name(),
                        "title": nc.label(),
                        "version": "1.0",
                    },
                    // Read by clients that surface server instructions to
                    // the model: the sentence, before any call is tried.
                    "instructions": message,
                }),
            )
        }
        // Liveness is not a capability: a client that pings to keep the
        // session must not decide the server died.
        "ping" => result(id, json!({})),
        "tools/list" => result(
            id,
            json!({ "tools": listed_tools(&nc.server.tools, nc.tools.as_deref(), message) }),
        ),
        _ => rpc_error(id, NOT_CONNECTED_CODE, message),
    })
}

/// The base a connect link is built on, as the PERSON will open it. The
/// gateway's fallback base is the origin the request arrived on, and the
/// request came from an agent container: on a host-run app that is
/// `host.docker.internal:<port>`, the agent-side alias of the laptop, which
/// no browser resolves. The person's browser reaches that same app as
/// `localhost:<port>` — the inverse of the rewrite fleet-render applies to a
/// loopback URL it hands to an agent. A configured base (verified domain,
/// AUTH_PUBLIC_URL) never names that alias and passes through unchanged.
pub fn browser_base(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_string();
    };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (authority, None),
    };
    if !host.eq_ignore_ascii_case("host.docker.internal") {
        return base.to_string();
    }
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    let path = if path.is_empty() {
        String::new()
    } else {
        format!("/{path}")
    };
    format!("{scheme}://localhost{port}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_mcp_jsonrpc::MCP_PROTOCOL_VERSION;

    fn server(label: &str, tools: Value) -> McpServer {
        McpServer {
            id: "00000000-0000-0000-0000-000000000002".into(),
            name: "notion".into(),
            label: label.into(),
            description: None,
            url: "https://mcp.notion.com/mcp".into(),
            headers_plain: Map::new(),
            headers_sealed: None,
            timeout_secs: None,
            enabled: true,
            all_agents: false,
            auth_mode: "per-user".into(),
            tools,
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
        }
    }

    fn nc(scope: ConnectScope, tools: Option<Vec<String>>, catalog: Value) -> NotConnected {
        NotConnected {
            scope,
            server: server("Notion", catalog),
            tools,
        }
    }

    const BASE: &str = "https://talaria.example.com/";

    #[test]
    fn the_three_sentences() {
        assert_eq!(
            not_connected_message(ConnectScope::PerUser, "Notion", BASE),
            "Your Notion account isn't connected to Talaria. Connect it at \
             https://talaria.example.com/settings/connections, then try again."
        );
        assert_eq!(
            not_connected_message(ConnectScope::Org, "Linear", BASE),
            "The workspace's Linear connection isn't set up yet. An admin connects it on the MCP \
             page at https://talaria.example.com/mcp/external."
        );
        assert_eq!(
            not_connected_message(ConnectScope::PerUserOnOrgAgent, "Notion", BASE),
            "Notion is a per-user server; only a personal assistant can use it."
        );
        // The base never doubles its slash, with or without one.
        assert_eq!(
            connect_url(ConnectScope::Org, "http://localhost:5273"),
            Some("http://localhost:5273/mcp/external".into())
        );
        assert_eq!(connect_url(ConnectScope::PerUserOnOrgAgent, BASE), None);
    }

    #[test]
    fn a_blank_label_falls_back_to_the_name() {
        let n = NotConnected {
            scope: ConnectScope::PerUserOnOrgAgent,
            server: server("  ", json!([])),
            tools: None,
        };
        assert_eq!(n.label(), "notion");
        assert_eq!(
            n.message(BASE),
            "notion is a per-user server; only a personal assistant can use it."
        );
    }

    #[test]
    fn initialize_is_a_valid_handshake_mirroring_the_server() {
        for scope in [
            ConnectScope::PerUser,
            ConnectScope::Org,
            ConnectScope::PerUserOnOrgAgent,
        ] {
            let n = nc(scope, None, json!([]));
            let rpc = json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "hermes" } },
            });
            let (status, body) = house_answer(&n, Some(&rpc), BASE);
            assert_eq!(status, 200);
            let body = body.expect("initialize is answered");
            assert_eq!(body["jsonrpc"], "2.0");
            assert_eq!(body["id"], 1);
            assert!(body.get("error").is_none(), "a handshake, not a refusal");
            let r = &body["result"];
            assert_eq!(
                r["protocolVersion"], "2025-06-18",
                "the client's revision echoes"
            );
            assert_eq!(r["capabilities"], json!({ "tools": {} }));
            assert_eq!(r["serverInfo"]["name"], "notion");
            assert_eq!(r["serverInfo"]["title"], "Notion");
            assert_eq!(r["instructions"], json!(n.message(BASE)));
        }
        // No revision named → ours.
        let n = nc(ConnectScope::Org, None, json!([]));
        let (_, body) = house_answer(&n, Some(&json!({ "id": 2, "method": "initialize" })), BASE);
        assert_eq!(
            body.unwrap()["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
    }

    #[test]
    fn the_initialized_notification_is_acked_and_ping_answers() {
        let n = nc(ConnectScope::PerUser, None, json!([]));
        let (status, body) = house_answer(
            &n,
            Some(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
            BASE,
        );
        assert_eq!((status, body), (202, None));
        let (status, body) = house_answer(
            &n,
            Some(&json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" })),
            BASE,
        );
        assert_eq!(status, 200);
        assert_eq!(body.unwrap()["result"], json!({}));
    }

    #[test]
    fn tools_list_is_the_cached_catalog_filtered_to_the_allowance() {
        let catalog = json!([
            { "name": "search", "description": "Search pages" },
            { "name": "create_page", "description": "Create a page" },
            { "name": "delete_page" },
        ]);
        // Everything allowed: all three, each a valid Tool with an inputSchema.
        let n = nc(ConnectScope::PerUser, None, catalog.clone());
        let (status, body) = house_answer(
            &n,
            Some(&json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/list" })),
            BASE,
        );
        assert_eq!(status, 200);
        let body = body.unwrap();
        assert_eq!(body["id"], "a");
        let tools = body["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["search", "create_page", "delete_page"]);
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
        assert_eq!(tools[0]["description"], "Search pages");
        assert!(tools[2].get("description").is_none());

        // An allowance narrows it, exactly like the live filter would.
        let n = nc(
            ConnectScope::Org,
            Some(vec!["search".into()]),
            catalog.clone(),
        );
        let (_, body) = house_answer(&n, Some(&json!({ "id": 3, "method": "tools/list" })), BASE);
        let tools = body.unwrap()["result"]["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "search");

        // Nothing to show (never discovered, or nothing allowed): the
        // placeholder carries the sentence.
        for n in [
            nc(ConnectScope::PerUser, None, json!([])),
            nc(ConnectScope::PerUser, Some(vec![]), catalog),
        ] {
            let (_, body) =
                house_answer(&n, Some(&json!({ "id": 4, "method": "tools/list" })), BASE);
            let tools = body.unwrap()["result"]["tools"].as_array().unwrap().clone();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0]["name"], PLACEHOLDER_TOOL);
            assert!(
                tools[0]["description"]
                    .as_str()
                    .unwrap()
                    .contains("/settings/connections")
            );
        }
    }

    #[test]
    fn tools_call_and_every_other_method_is_the_32001_sentence() {
        for (scope, needle) in [
            (ConnectScope::PerUser, "Your Notion account isn't connected"),
            (
                ConnectScope::Org,
                "The workspace's Notion connection isn't set up yet",
            ),
            (
                ConnectScope::PerUserOnOrgAgent,
                "only a personal assistant can use it",
            ),
        ] {
            let n = nc(scope, None, json!([{ "name": "search" }]));
            for rpc in [
                json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "search", "arguments": {} } }),
                json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": PLACEHOLDER_TOOL } }),
                json!({ "jsonrpc": "2.0", "id": 7, "method": "resources/list" }),
            ] {
                let (status, body) = house_answer(&n, Some(&rpc), BASE);
                assert_eq!(
                    status, 200,
                    "the gate's framing: a JSON-RPC error over HTTP 200"
                );
                let body = body.unwrap();
                assert_eq!(body["jsonrpc"], "2.0");
                assert_eq!(body["id"], 7);
                assert_eq!(body["error"]["code"], NOT_CONNECTED_CODE);
                let msg = body["error"]["message"].as_str().unwrap();
                assert!(msg.contains(needle), "{msg}");
                assert_eq!(msg, n.message(BASE));
            }
        }
        // An unparseable body still gets the sentence, id null.
        let n = nc(ConnectScope::Org, None, json!([]));
        let (status, body) = house_answer(&n, None, BASE);
        assert_eq!(status, 200);
        let body = body.unwrap();
        assert_eq!(body["id"], Value::Null);
        assert_eq!(body["error"]["code"], NOT_CONNECTED_CODE);
    }

    #[test]
    fn a_message_without_an_id_is_a_notification_whatever_its_method() {
        let n = nc(ConnectScope::PerUser, None, json!([{ "name": "search" }]));
        for rpc in [
            json!({ "jsonrpc": "2.0", "method": "tools/call", "params": { "name": "search" } }),
            json!({ "jsonrpc": "2.0", "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": {} }),
        ] {
            assert_eq!(house_answer(&n, Some(&rpc), BASE), (202, None), "{rpc}");
        }
        // An explicit null id is still a request (JSON-RPC discourages it,
        // but it is answered).
        let (status, body) = house_answer(
            &n,
            Some(&json!({ "jsonrpc": "2.0", "id": null, "method": "tools/call" })),
            BASE,
        );
        assert_eq!(status, 200);
        assert_eq!(body.unwrap()["error"]["code"], NOT_CONNECTED_CODE);
    }

    #[test]
    fn a_batch_gets_an_array_of_its_requests_replies() {
        let n = nc(ConnectScope::Org, None, json!([{ "name": "search" }]));
        let batch = json!([
            { "jsonrpc": "2.0", "id": 1, "method": "tools/list" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "search" } },
            7,
        ]);
        let (status, body) = house_answer(&n, Some(&batch), BASE);
        assert_eq!(status, 200);
        let replies = body.unwrap();
        let replies = replies.as_array().expect("a batch answers an array");
        assert_eq!(replies.len(), 3, "the notification earns no reply");
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[0]["result"]["tools"][0]["name"], "search");
        assert_eq!(replies[1]["id"], 2);
        assert_eq!(replies[1]["error"]["code"], NOT_CONNECTED_CODE);
        assert_eq!(replies[2]["id"], Value::Null);
        assert_eq!(replies[2]["error"]["code"], -32600);
        // Only notifications: no body at all.
        let (status, body) = house_answer(
            &n,
            Some(&json!([{ "jsonrpc": "2.0", "method": "notifications/initialized" }])),
            BASE,
        );
        assert_eq!((status, body), (202, None));
        // An empty batch is one Invalid Request.
        let (status, body) = house_answer(&n, Some(&json!([])), BASE);
        assert_eq!(status, 200);
        let body = body.unwrap();
        assert!(body.is_object());
        assert_eq!(body["error"]["code"], -32600);
    }

    #[test]
    fn initialize_negotiates_an_unknown_revision_down_to_ours() {
        let n = nc(ConnectScope::PerUser, None, json!([]));
        for (asked, answered) in [
            ("2099-01-01", MCP_PROTOCOL_VERSION),
            ("not-a-date", MCP_PROTOCOL_VERSION),
            ("2024-11-05", "2024-11-05"),
            ("2025-03-26", "2025-03-26"),
            ("2025-06-18", "2025-06-18"),
            ("2025-11-25", "2025-11-25"),
        ] {
            let rpc =
                json!({ "id": 1, "method": "initialize", "params": { "protocolVersion": asked } });
            let (_, body) = house_answer(&n, Some(&rpc), BASE);
            assert_eq!(
                body.unwrap()["result"]["protocolVersion"],
                answered,
                "{asked}"
            );
        }
    }

    #[test]
    fn the_connect_link_is_one_a_browser_opens() {
        // The agent's view of a host-run app becomes the person's.
        assert_eq!(
            browser_base("http://host.docker.internal:5303"),
            "http://localhost:5303"
        );
        assert_eq!(
            browser_base("http://HOST.DOCKER.INTERNAL:5273/"),
            "http://localhost:5273"
        );
        assert_eq!(
            browser_base("http://host.docker.internal"),
            "http://localhost"
        );
        // A configured base passes through untouched (bar a trailing slash).
        for (given, kept) in [
            ("https://talaria.example.com", "https://talaria.example.com"),
            (
                "https://talaria.example.com/",
                "https://talaria.example.com",
            ),
            ("http://localhost:5273", "http://localhost:5273"),
            ("http://10.0.0.4:8080", "http://10.0.0.4:8080"),
            (
                "http://host.docker.internal.example.com",
                "http://host.docker.internal.example.com",
            ),
        ] {
            assert_eq!(browser_base(given), kept);
        }
        // And the sentence carries the browser's address.
        assert_eq!(
            not_connected_message(
                ConnectScope::PerUser,
                "Notion",
                &browser_base("http://host.docker.internal:5303")
            ),
            "Your Notion account isn't connected to Talaria. Connect it at \
             http://localhost:5303/settings/connections, then try again."
        );
    }
}

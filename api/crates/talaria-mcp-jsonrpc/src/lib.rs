// JSON-RPC 2.0 over MCP — the wire shape and the method envelope every
// in-process MCP surface in this repo speaks: the whole
// initialize/initialized/ping/tools-list/tools-call switch, identical across
// dispatchers except for the server's name and what a tool call actually
// does, which is exactly what a callback is for.
//
// A leaf: the registry, the app dispatcher, and the workbench dispatcher all
// consume it, and none may import the others.

use std::future::Future;
use std::pin::Pin;

use axum::http::StatusCode;
use serde_json::{Map as JsonMap, Value, json};

/// The one MCP protocol revision Talaria speaks as a SERVER — the fallback
/// answer when a client doesn't name one (a single literal pinned for every
/// direction; bumping it is a protocol decision).
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// `result(id, res)` — the id echoes whatever the caller sent, or null when
/// absent (an explicit null stays null; a JSON-RPC notification's missing id
/// becomes null).
pub fn result(id: &Value, res: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id_or_null(id), "result": res })
}

/// `rpcError(id, code, message)`.
pub fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id_or_null(id), "error": { "code": code, "message": message } })
}

fn id_or_null(id: &Value) -> Value {
    if id.is_null() {
        Value::Null
    } else {
        id.clone()
    }
}

/// One listed tool: its name (what tools/call matches on) and the entry
/// tools/list answers with (already shaped by the surface).
pub struct ListedTool {
    pub name: String,
    pub entry: Value,
}

/// What one tool call produced. `Ok` is a value the model reads as JSON
/// text; `Fail` is the tool ANSWERING with a failure (`Error: …`); `Throw`
/// is the call itself blowing up (`error: …`) — the exception's message is
/// tool output the model should see and adapt to, not a transport failure.
pub enum ToolOutcome {
    Ok(Value),
    Fail(String),
    Throw(String),
}

/// The async call shape `dispatch_jsonrpc` takes: `(tool name, arguments)`.
pub type CallFn<'a> = dyn FnMut(&str, JsonMap<String, Value>) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>
    + Send
    + 'a;

/// The method envelope for an MCP tools surface. `tools` arrives ALREADY
/// filtered to what the caller may use (the gateway's resolution, enforced
/// again by the in-process dispatcher). Returns the status and body the HTTP
/// route answers with — body None is the 202/no-content notification ack.
pub async fn dispatch_jsonrpc(
    rpc: &Value,
    tools: Vec<ListedTool>,
    server_name: &str,
    mut call: Box<CallFn<'_>>,
) -> (StatusCode, Option<Value>) {
    let id = rpc.get("id").cloned().unwrap_or(Value::Null);
    let method = rpc
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("undefined");
    let params = rpc.get("params").cloned().unwrap_or(Value::Null);
    let pobj = params.as_object().cloned().unwrap_or_default();
    match method {
        "initialize" => {
            let protocol = pobj
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            (
                StatusCode::OK,
                Some(result(
                    &id,
                    json!({
                        "protocolVersion": protocol,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": server_name, "version": "1.0" },
                    }),
                )),
            )
        }
        // A notification carries no id and expects no reply — acknowledged,
        // not answered.
        "notifications/initialized" => (StatusCode::ACCEPTED, None),
        "ping" => (StatusCode::OK, Some(result(&id, json!({})))),
        "tools/list" => (
            StatusCode::OK,
            Some(result(
                &id,
                json!({ "tools": tools.iter().map(|t| t.entry.clone()).collect::<Vec<_>>() }),
            )),
        ),
        "tools/call" => {
            let name = pobj.get("name").and_then(Value::as_str);
            let Some(tool) = tools.iter().find(|t| Some(t.name.as_str()) == name) else {
                // a missing name renders as the literal "undefined" — the
                // wire shape, not "".
                let shown = name.unwrap_or("undefined");
                return (
                    StatusCode::OK,
                    Some(rpc_error(
                        &id,
                        -32602,
                        &format!("tool \"{shown}\" is not available here"),
                    )),
                );
            };
            let args = pobj
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let tool_name = tool.name.clone();
            match call(&tool_name, args).await {
                ToolOutcome::Ok(value) => (
                    StatusCode::OK,
                    Some(result(
                        &id,
                        json!({ "content": [{ "type": "text", "text": serde_json::to_string(&value).unwrap_or_else(|_| "null".into()) }] }),
                    )),
                ),
                ToolOutcome::Fail(text) => (
                    StatusCode::OK,
                    Some(result(
                        &id,
                        json!({ "content": [{ "type": "text", "text": format!("Error: {text}") }], "isError": true }),
                    )),
                ),
                ToolOutcome::Throw(message) => (
                    StatusCode::OK,
                    Some(result(
                        &id,
                        json!({ "content": [{ "type": "text", "text": format!("error: {message}") }], "isError": true }),
                    )),
                ),
            }
        }
        _ => (
            StatusCode::OK,
            Some(rpc_error(
                &id,
                -32601,
                &format!("method \"{method}\" not supported"),
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_call<'a>() -> Box<CallFn<'a>> {
        Box::new(|_, _| {
            Box::pin(async { ToolOutcome::Ok(Value::Null) })
                as Pin<Box<dyn Future<Output = ToolOutcome> + Send>>
        })
    }

    fn tools() -> Vec<ListedTool> {
        vec![
            ListedTool {
                name: "doctor".into(),
                entry: json!({ "name": "doctor" }),
            },
            ListedTool {
                name: "start_job".into(),
                entry: json!({ "name": "start_job" }),
            },
        ]
    }

    #[tokio::test]
    async fn initialize_echoes_the_callers_protocol_else_the_pinned_one() {
        let rpc = json!({ "jsonrpc": "2.0", "id": 7, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } });
        let (status, body) =
            dispatch_jsonrpc(&rpc, tools(), "talaria-workbench", never_call()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body.unwrap()["result"]["protocolVersion"],
            json!("2025-06-18")
        );

        let rpc = json!({ "jsonrpc": "2.0", "id": 7, "method": "initialize" });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "talaria-workbench", never_call()).await;
        assert_eq!(
            body.unwrap()["result"]["protocolVersion"],
            json!(MCP_PROTOCOL_VERSION)
        );
        // serverInfo rides the surface's own name.
        let rpc = json!({ "id": 1, "method": "initialize" });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "app-x", never_call()).await;
        assert_eq!(
            body.unwrap()["result"]["serverInfo"]["name"],
            json!("app-x")
        );
    }

    #[tokio::test]
    async fn a_notification_is_acknowledged_not_answered() {
        let rpc = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let (status, body) = dispatch_jsonrpc(&rpc, tools(), "s", never_call()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(body.is_none());
    }

    #[tokio::test]
    async fn ids_echo_verbatim_and_absent_becomes_null() {
        let rpc = json!({ "id": "abc", "method": "ping" });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", never_call()).await;
        assert_eq!(body.unwrap()["id"], json!("abc"));

        let rpc = json!({ "method": "ping" });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", never_call()).await;
        assert_eq!(body.unwrap()["id"], Value::Null);
    }

    #[tokio::test]
    async fn unknown_tool_and_unknown_method_carry_their_codes() {
        let rpc = json!({ "id": 2, "method": "tools/call", "params": { "name": "nope" } });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", never_call()).await;
        let body = body.unwrap();
        assert_eq!(body["error"]["code"], json!(-32602));
        assert_eq!(
            body["error"]["message"],
            json!("tool \"nope\" is not available here")
        );

        // No name at all: the literal "undefined" goes into the message.
        let rpc = json!({ "id": 2, "method": "tools/call", "params": {} });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", never_call()).await;
        assert_eq!(
            body.unwrap()["error"]["message"],
            json!("tool \"undefined\" is not available here")
        );

        let rpc = json!({ "id": 3, "method": "resources/list" });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", never_call()).await;
        let body = body.unwrap();
        assert_eq!(body["error"]["code"], json!(-32601));
        assert_eq!(
            body["error"]["message"],
            json!("method \"resources/list\" not supported")
        );
    }

    #[tokio::test]
    async fn the_three_call_outcomes_shape_three_different_bodies() {
        let rpc = json!({ "id": 9, "method": "tools/call", "params": { "name": "doctor", "arguments": { "x": 1 } } });

        let ok = Box::new(|_n: &str, _a: JsonMap<String, Value>| {
            Box::pin(async { ToolOutcome::Ok(json!({ "checks": [] })) })
                as Pin<Box<dyn Future<Output = ToolOutcome> + Send>>
        });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", ok).await;
        let body = body.unwrap();
        assert_eq!(body["result"]["isError"], Value::Null);
        assert_eq!(
            body["result"]["content"][0]["text"],
            json!(serde_json::to_string(&json!({ "checks": [] })).unwrap())
        );

        let fail = Box::new(|_n: &str, _a: JsonMap<String, Value>| {
            Box::pin(async { ToolOutcome::Fail("unknown agent".into()) })
                as Pin<Box<dyn Future<Output = ToolOutcome> + Send>>
        });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", fail).await;
        let body = body.unwrap();
        assert_eq!(body["result"]["isError"], json!(true));
        assert_eq!(
            body["result"]["content"][0]["text"],
            json!("Error: unknown agent")
        );

        let throw = Box::new(|_n: &str, _a: JsonMap<String, Value>| {
            Box::pin(async { ToolOutcome::Throw("GitHub is not connected".into()) })
                as Pin<Box<dyn Future<Output = ToolOutcome> + Send>>
        });
        let (_, body) = dispatch_jsonrpc(&rpc, tools(), "s", throw).await;
        let body = body.unwrap();
        assert_eq!(body["result"]["isError"], json!(true));
        assert_eq!(
            body["result"]["content"][0]["text"],
            json!("error: GitHub is not connected")
        );
    }
}

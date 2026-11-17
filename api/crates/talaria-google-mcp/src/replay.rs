// The approved replay of a held call: one `tools/call` on the product's
// endpoint, with the bearer of the principal the call was queued for, sent
// alone — no `initialize`. Google Cloud's MCP overview names protocol
// 2026-07-28 (stateless), and Google answered a `tools/list` sent with no
// `initialize` when probed (2026-11-12 and 2026-11-17).
//
// What comes back is read strictly. Only a plain completed tool result is
// DONE: HTTP 2xx, one JSON-RPC response to this request carrying a `result`
// with a `content` array, no `isError: true`, no sign of a multi-round-trip
// request, and nothing else in the stream asking the client anything.
// Everything else fails the approval, quoting what Google sent:
//   · an HTTP error, a JSON-RPC error, or a result with `isError` — Google's
//     words, read the way the check reads them (`read_check_answer`);
//   · a request from the server inside the stream (`elicitation/create`,
//     `sampling/createMessage`, `roots/list`, any method with an id) — the
//     owner's decision D6: every elicitation and MRTR fails the replay until
//     one has been captured live, and only then may a confirm-only one be
//     answered in the approver's name;
//   · a result asking for more input instead of carrying content (MRTR's
//     `inputRequests`, a `resultType` other than `complete`) — D6 again;
//   · anything else: no response to this request, a batch, a body that is
//     not JSON-RPC.
// NONE of these answers has been captured from Google: nobody here is
// enrolled in the Developer Preview, so no authenticated call was ever made.
// The 401 bodies in the tests are Google's (fixtures/, captured 2026-11-17);
// every accepted, elicitation and MRTR shape there is the MCP specification's
// and says so.
//
// The bearer goes in one header to one URL and nowhere else: never logged,
// never in an error string, never in what is stored.

use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use talaria_google_errors::quote_raw;

/// The whole-request budget for one replay: a person is waiting on it.
const REPLAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// The most of Google's answer the replay reads.
const REPLAY_MAX_BYTES: usize = 4 * 1024 * 1024;
/// The most of a tool result the approval row keeps.
const STORED_RESULT_MAX_BYTES: usize = 32 * 1024;
/// The JSON-RPC id the replay sends, which the answer must carry back.
const REPLAY_ID: i64 = 1;

/// What Google answered an approved replay.
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayAnswer {
    /// Google ran it. The tool result as Google sent it: `content`, and
    /// `structuredContent` when there was one.
    Done(Value),
    /// Google refused, or the tool reported a failure: the HTTP status and
    /// Google's words (None when it sent none).
    Failed { status: u16, text: Option<String> },
    /// Google asked for more before it would run the tool — an elicitation
    /// (`how` is the method it sent) or a multi-round-trip request (`how` is
    /// `inputRequests`, or the result type it named). Talaria answers none.
    AskedForInput { how: String, text: Option<String> },
    /// Google's 2xx answer was not a completed tool result Talaria can read:
    /// the status and the body, folded and capped.
    Unreadable { status: u16, text: Option<String> },
}

/// Send `tool` with `arguments` to `endpoint` with `bearer`, on `client` —
/// the relay client (no redirects, the dialled address re-checked) in
/// production. `Err` is a call that never got Google's answer: the request
/// or the read of its body failed. For a write that is ambiguous — it may
/// have run — and the caller must say so. The error never names the bearer.
pub async fn call_tool(
    client: &reqwest::Client,
    endpoint: &str,
    bearer: &str,
    tool: &str,
    arguments: &Value,
) -> Result<ReplayAnswer, String> {
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": REPLAY_ID,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    });
    let res = client
        .post(endpoint)
        .bearer_auth(bearer)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&rpc)
        .timeout(REPLAY_TIMEOUT)
        .send()
        .await
        .map_err(|e| transport_error("did not complete", endpoint, &e))?;
    let status = res.status().as_u16();
    let mut body: Vec<u8> = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| transport_error("broke off", endpoint, &e))?;
        let room = REPLAY_MAX_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= REPLAY_MAX_BYTES {
            break;
        }
    }
    Ok(read_replay(status, &String::from_utf8_lossy(&body)))
}

/// A transport failure in words that name the endpoint, never the request
/// (reqwest's error can carry the URL; it never carries headers).
fn transport_error(what: &str, endpoint: &str, e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else if e.is_redirect() {
        "was redirected, which is never followed"
    } else {
        "failed"
    };
    format!("the call to {endpoint} {what} ({kind})")
}

/// Google's answer to a replay, read (see the header). Pure.
pub fn read_replay(status: u16, body: &str) -> ReplayAnswer {
    let refused = |text: Option<String>| ReplayAnswer::Failed { status, text };
    if !(200..300).contains(&status) {
        return refused(crate::read_check_answer(status, body).text);
    }
    let unreadable = || ReplayAnswer::Unreadable {
        status,
        text: (!body.trim().is_empty()).then(|| quote_raw(body.trim())),
    };
    let Some(messages) = messages_of(body) else {
        return unreadable();
    };
    // A request from the server — anything with a method AND an id — asks
    // the client something. Talaria answers none of them (D6).
    if let Some(ask) = messages
        .iter()
        .find(|m| m.get("method").is_some() && m.get("id").is_some())
    {
        let how = ask
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("a request")
            .to_string();
        return ReplayAnswer::AskedForInput {
            how,
            text: ask.get("params").and_then(asked_words),
        };
    }
    let Some(answer) = messages.iter().find(|m| {
        m.get("method").is_none()
            && m.get("id").and_then(Value::as_i64) == Some(REPLAY_ID)
            && (m.get("result").is_some() || m.get("error").is_some())
    }) else {
        return unreadable();
    };
    if answer.get("error").is_some() {
        return refused(crate::read_check_answer(status, &answer.to_string()).text);
    }
    let Some(result) = answer.get("result").and_then(Value::as_object) else {
        return unreadable();
    };
    if let Some(inputs) = result.get("inputRequests") {
        return ReplayAnswer::AskedForInput {
            how: "inputRequests".into(),
            text: asked_words(inputs),
        };
    }
    if let Some(kind) = result.get("resultType")
        && kind.as_str() != Some("complete")
    {
        return ReplayAnswer::AskedForInput {
            how: format!("resultType {kind}"),
            text: asked_words(&Value::Object(result.clone())),
        };
    }
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return refused(crate::read_check_answer(status, &answer.to_string()).text);
    }
    if !result.get("content").is_some_and(Value::is_array) {
        return unreadable();
    }
    let mut kept = Map::new();
    for key in ["content", "structuredContent"] {
        if let Some(v) = result.get(key).filter(|v| !v.is_null()) {
            kept.insert(key.into(), v.clone());
        }
    }
    ReplayAnswer::Done(Value::Object(kept))
}

/// The JSON-RPC messages in a body: one JSON object, or the `data:` of each
/// SSE event (events end at a blank line; an event's data lines are joined
/// with newlines, as the SSE spec reads them). None when the body is neither,
/// or is a batch, or an event's data does not parse.
fn messages_of(body: &str) -> Option<Vec<Value>> {
    let text = body.replace("\r\n", "\n");
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        let one: Value = serde_json::from_str(trimmed).ok()?;
        return one.is_object().then(|| vec![one]);
    }
    let mut out = Vec::new();
    for event in trimmed.split("\n\n") {
        let data: Vec<&str> = event
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(|d| d.strip_prefix(' ').unwrap_or(d))
            .collect();
        if data.is_empty() {
            continue;
        }
        let msg: Value = serde_json::from_str(data.join("\n").trim()).ok()?;
        if !msg.is_object() {
            return None;
        }
        out.push(msg);
    }
    (!out.is_empty()).then_some(out)
}

/// Google's words in an ask: the first `message` string anywhere in it (an
/// elicitation's `params.message`), else the whole of it, folded and capped.
fn asked_words(v: &Value) -> Option<String> {
    fn first_message(v: &Value) -> Option<&str> {
        match v {
            Value::Object(o) => o
                .get("message")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .or_else(|| o.values().find_map(first_message)),
            Value::Array(a) => a.iter().find_map(first_message),
            _ => None,
        }
    }
    first_message(v)
        .map(|m| quote_raw(m.trim()))
        .or_else(|| (!v.is_null()).then(|| quote_raw(&v.to_string())))
}

/// What the approval row keeps of a done result: the result itself, or —
/// past `STORED_RESULT_MAX_BYTES` — its text content cut short, marked
/// `truncated`. Never anything but what Google returned.
pub fn stored_result(result: &Value) -> Value {
    let raw = result.to_string();
    if raw.len() <= STORED_RESULT_MAX_BYTES {
        return result.clone();
    }
    let text: String = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let mut cut = text;
    if cut.len() > STORED_RESULT_MAX_BYTES {
        let mut end = STORED_RESULT_MAX_BYTES;
        while !cut.is_char_boundary(end) {
            end -= 1;
        }
        cut.truncate(end);
    }
    json!({ "content": [{ "type": "text", "text": cut }], "truncated": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Google's own refusals, captured from https://gmailmcp.googleapis.com/
    // mcp/v1 on 2026-11-17 (`tools/call list_labels`, a bogus bearer and no
    // credential): HTTP 401 with a JSON-RPC result carrying `isError`.
    const BOGUS_BEARER: &str = include_str!("../fixtures/list-labels-401-bogus.json");
    const NO_CREDENTIAL: &str = include_str!("../fixtures/list-labels-401-nocred.json");
    // The same server, an unknown method: HTTP 200 with a JSON-RPC error.
    const UNKNOWN_METHOD: &str = include_str!("../fixtures/unknown-method-200.json");

    /// MCP's CallToolResult shape (the specification's, NOT a Google
    /// capture: no accepted call has been made here).
    const SPEC_RESULT: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"Event created"}],"structuredContent":{"id":"ev1"},"isError":false}}"#;

    #[test]
    fn googles_refusals_fail_the_replay_in_googles_words() {
        assert_eq!(
            read_replay(401, BOGUS_BEARER),
            ReplayAnswer::Failed {
                status: 401,
                text: Some("Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.".into()),
            }
        );
        match read_replay(401, NO_CREDENTIAL) {
            ReplayAnswer::Failed {
                status: 401,
                text: Some(t),
            } => assert!(t.starts_with("Request is missing required authentication credential.")),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            read_replay(200, UNKNOWN_METHOD),
            ReplayAnswer::Failed {
                status: 200,
                text: Some("Method not supported".into()),
            },
            "an HTTP 200 carrying a JSON-RPC error is no success"
        );
        // A tool failure under HTTP 200: Google's 401 body's shape, served 200.
        match read_replay(200, BOGUS_BEARER) {
            ReplayAnswer::Failed {
                status: 200,
                text: Some(t),
            } => {
                assert!(t.starts_with("Request had invalid authentication credentials."))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            read_replay(
                503,
                "<html><title>Error 503 (Service Unavailable)!!1</title></html>"
            ),
            ReplayAnswer::Failed {
                status: 503,
                text: Some("Error 503 (Service Unavailable)!!1".into()),
            }
        );
    }

    #[test]
    fn only_a_plain_completed_result_is_done() {
        assert_eq!(
            read_replay(200, SPEC_RESULT),
            ReplayAnswer::Done(json!({
                "content": [{ "type": "text", "text": "Event created" }],
                "structuredContent": { "id": "ev1" },
            }))
        );
        // The same, SSE-framed, with a progress notification ahead of it.
        let sse = format!(
            "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{\"progress\":1}}}}\n\nevent: message\ndata: {SPEC_RESULT}\n\n"
        );
        assert!(matches!(read_replay(200, &sse), ReplayAnswer::Done(_)));
        // CRLF framing reads the same.
        assert!(matches!(
            read_replay(200, &sse.replace('\n', "\r\n")),
            ReplayAnswer::Done(_)
        ));
        // A result with no content array, a response to another id, a batch,
        // a body that is no JSON-RPC, an empty body: unreadable, never done.
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":"x"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[]}}"#,
            r#"[{"jsonrpc":"2.0","id":1,"result":{"content":[]}}]"#,
            "<html><title>OK</title></html>",
            "",
            "data: not json\n\n",
        ] {
            assert!(
                matches!(
                    read_replay(200, body),
                    ReplayAnswer::Unreadable { status: 200, .. }
                ),
                "{body}"
            );
        }
    }

    /// D6: an elicitation inside the stream fails the replay, with the
    /// server's own message. The shape is MCP's `elicitation/create`
    /// (the specification's, NOT a Google capture).
    #[test]
    fn an_elicitation_fails_the_replay_with_its_own_words() {
        let ask = r#"{"jsonrpc":"2.0","id":"e1","method":"elicitation/create","params":{"message":"Confirm sending this message to 3 external recipients?","requestedSchema":{"type":"object","properties":{"confirm":{"type":"boolean"}}}}}"#;
        let sse = format!("event: message\ndata: {ask}\n\nevent: message\ndata: {SPEC_RESULT}\n\n");
        assert_eq!(
            read_replay(200, &sse),
            ReplayAnswer::AskedForInput {
                how: "elicitation/create".into(),
                text: Some("Confirm sending this message to 3 external recipients?".into()),
            },
            "even with a result after it, the ask decides"
        );
        // Any other request from the server is an ask too.
        let sampling = r#"{"jsonrpc":"2.0","id":7,"method":"sampling/createMessage","params":{}}"#;
        match read_replay(200, &format!("data: {sampling}\n\n")) {
            ReplayAnswer::AskedForInput { how, text } => {
                assert_eq!(how, "sampling/createMessage");
                assert_eq!(text.as_deref(), Some("{}"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// D6: a multi-round-trip request — a result that asks for input
    /// instead of carrying content — fails the replay. The shapes are the
    /// stateless protocol's as Talaria reads it, NOT a Google capture.
    #[test]
    fn a_multi_round_trip_request_fails_the_replay() {
        let mrtr = r#"{"jsonrpc":"2.0","id":1,"result":{"inputRequests":{"confirm":{"method":"elicitation/create","params":{"message":"Approve deleting 1 event?"}}},"requestState":"opaque"}}"#;
        assert_eq!(
            read_replay(200, mrtr),
            ReplayAnswer::AskedForInput {
                how: "inputRequests".into(),
                text: Some("Approve deleting 1 event?".into()),
            }
        );
        let incomplete =
            r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"incomplete","content":[]}}"#;
        match read_replay(200, incomplete) {
            ReplayAnswer::AskedForInput { how, .. } => assert_eq!(how, "resultType \"incomplete\""),
            other => panic!("{other:?}"),
        }
        // `complete` is a completed result.
        let complete =
            r#"{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","content":[]}}"#;
        assert!(matches!(read_replay(200, complete), ReplayAnswer::Done(_)));
    }

    #[test]
    fn the_stored_result_is_capped_and_says_so() {
        let small = json!({ "content": [{ "type": "text", "text": "ok" }] });
        assert_eq!(stored_result(&small), small);
        let big_text = "é".repeat(STORED_RESULT_MAX_BYTES);
        let big = json!({ "content": [{ "type": "text", "text": big_text }], "structuredContent": { "x": 1 } });
        let kept = stored_result(&big);
        assert_eq!(kept["truncated"], true);
        let text = kept["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= STORED_RESULT_MAX_BYTES);
        assert!(big_text.starts_with(text));
    }

    /// The bearer reaches the endpoint in the Authorization header and
    /// nothing else; the request is one stateless `tools/call`.
    #[tokio::test]
    async fn the_replay_sends_one_tools_call_with_the_bearer_in_its_header_only() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let served = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len: usize = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if buf.len() >= head_end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let reply = SPEC_RESULT;
            let out = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            sock.write_all(out.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf).to_string()
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let answer = call_tool(
            &client,
            &format!("http://{addr}/mcp/v1"),
            "ya29.secret-bearer",
            "create_event",
            &json!({ "summary": "Standup" }),
        )
        .await
        .unwrap();
        assert!(matches!(answer, ReplayAnswer::Done(_)), "{answer:?}");
        let request = served.await.unwrap();
        let (head, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("POST /mcp/v1 HTTP/1.1"), "{head}");
        assert!(
            head.lines()
                .any(|l| l.eq_ignore_ascii_case("authorization: Bearer ya29.secret-bearer")),
            "{head}"
        );
        assert!(!body.contains("ya29.secret-bearer"));
        let rpc: Value = serde_json::from_str(body).unwrap();
        assert_eq!(rpc["method"], "tools/call");
        assert_eq!(rpc["params"]["name"], "create_event");
        assert_eq!(rpc["params"]["arguments"]["summary"], "Standup");
    }

    #[tokio::test]
    async fn a_call_that_never_got_an_answer_is_an_error_naming_no_bearer() {
        // Nothing listens on this port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let client = reqwest::Client::new();
        let e = call_tool(
            &client,
            &format!("http://{addr}/mcp/v1"),
            "ya29.secret-bearer",
            "create_event",
            &json!({}),
        )
        .await
        .unwrap_err();
        assert!(e.contains("did not complete"), "{e}");
        assert!(!e.contains("ya29"), "{e}");
    }
}

// Google's Workspace MCP servers, as Talaria knows them: the product catalog
// (each server's endpoint, and the two services a Google Cloud project enables
// for it), "Check with Google" (`check_agentic`), and the pack — the servers
// as registry rows behind the MCP gateway (`pack`), what an agent may run on
// them (`policy`), their catalogs with Google's per-tool scopes (`catalog`),
// and the approved replay of a call a person was asked about (`replay`).
//
// Google's notice (Workspace developer release notes, October 12, 2026):
// "Starting November 11, 2026, Google Workspace MCP servers will reject
// requests from any app or integration whose OAuth client is not marked as
// agentic by selecting Use this client for an AI-powered agent." Google
// exposes no API and no token claim for that mark, so a call these servers
// ACCEPT, made with a token one client minted, is the only provider-side
// evidence that the client carries it. `check_agentic` makes the cheapest
// call there is — Gmail's `list_labels`, one unit of the Gmail MCP server's
// query-cost quota — and reports what Google answered, verbatim.
//
// A refusal is never read for a class. Google's body for a client not marked
// agentic has not been captured, and the same call also fails, in Google's
// own words, for a project that has not enabled the Gmail MCP service or is
// not in the Developer Preview, and for a token without a Gmail scope. Only
// an accepted call says anything about the mark.
//
// The servers are stateless: Google Cloud's MCP overview names protocol
// 2026-07-28, which has no `initialize` handshake and no `Mcp-Session-Id`, so
// a `tools/call` goes alone. A refused call comes back as a JSON-RPC
// *result* with `isError: true`, the HTTP status carrying the auth failure —
// the bodies under fixtures/ were captured from Google's servers on
// 2026-11-17 with no credential and with a bogus bearer.
//
// Nothing here holds a credential or reads the database: the caller vends
// the token (talaria-google-agent's one door) and stores what it learns.

pub mod catalog;
pub mod pack;
pub mod policy;
pub mod replay;

use reqwest::header::ACCEPT;
use serde_json::{Value, json};
use talaria_gateway::provider::http;
use talaria_google_errors::{quote_raw, upstream_reason};
use talaria_mcp_jsonrpc::parse_mcp_response;

/// One Google Workspace MCP server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Product {
    /// Talaria's short name for it.
    pub id: &'static str,
    /// Google's product name.
    pub name: &'static str,
    /// The Streamable HTTP endpoint.
    pub endpoint: &'static str,
    /// The MCP service a project enables (`gcloud services enable …`).
    pub mcp_service: &'static str,
    /// The product's REST API, which the project enables as well. None when
    /// the server spans several products.
    pub rest_service: Option<&'static str>,
}

/// Gmail's server: the one `check_agentic` calls.
pub const GMAIL: Product = Product {
    id: "gmail",
    name: "Gmail",
    endpoint: "https://gmailmcp.googleapis.com/mcp/v1",
    mcp_service: "gmailmcp.googleapis.com",
    rest_service: Some("gmail.googleapis.com"),
};

/// Every server Google documents with a live catalog (configure-mcp-servers,
/// read 2026-11-12). Classroom is left out: its live `tools/list` returned no
/// tools. People has no MCP service of its own; Universal Search
/// (`workspacemcp`) searches whichever of the others the person granted.
pub const PRODUCTS: [Product; 9] = [
    GMAIL,
    Product {
        id: "drive",
        name: "Google Drive",
        endpoint: "https://drivemcp.googleapis.com/mcp/v1",
        mcp_service: "drivemcp.googleapis.com",
        rest_service: Some("drive.googleapis.com"),
    },
    Product {
        id: "calendar",
        name: "Google Calendar",
        endpoint: "https://calendarmcp.googleapis.com/mcp/v1",
        mcp_service: "calendarmcp.googleapis.com",
        rest_service: Some("calendar-json.googleapis.com"),
    },
    Product {
        id: "docs",
        name: "Google Docs",
        endpoint: "https://docsmcp.googleapis.com/mcp/v1",
        mcp_service: "docsmcp.googleapis.com",
        rest_service: Some("docs.googleapis.com"),
    },
    Product {
        id: "sheets",
        name: "Google Sheets",
        endpoint: "https://sheetsmcp.googleapis.com/mcp/v1",
        mcp_service: "sheetsmcp.googleapis.com",
        rest_service: Some("sheets.googleapis.com"),
    },
    Product {
        id: "slides",
        name: "Google Slides",
        endpoint: "https://slidesmcp.googleapis.com/mcp/v1",
        mcp_service: "slidesmcp.googleapis.com",
        rest_service: Some("slides.googleapis.com"),
    },
    Product {
        id: "chat",
        name: "Google Chat",
        endpoint: "https://chatmcp.googleapis.com/mcp/v1",
        mcp_service: "chatmcp.googleapis.com",
        rest_service: Some("chat.googleapis.com"),
    },
    Product {
        id: "people",
        name: "People",
        endpoint: "https://people.googleapis.com/mcp/v1",
        mcp_service: "people.googleapis.com",
        rest_service: Some("people.googleapis.com"),
    },
    Product {
        id: "search",
        name: "Universal Search",
        endpoint: "https://workspacemcp.googleapis.com/mcp/v1",
        mcp_service: "workspacemcp.googleapis.com",
        rest_service: None,
    },
];

/// The tool the check calls: read-only, no arguments, and the cheapest in
/// Gmail's MCP quota table ("list_labels 1").
pub const CHECK_TOOL: &str = "list_labels";

/// The whole-request budget for the check. An admin is waiting on it.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What Google answered the check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckAnswer {
    /// Google accepted the call: HTTP 2xx and a JSON-RPC result that is not
    /// `isError`. The only answer that says anything about the mark.
    pub ok: bool,
    /// The HTTP status.
    pub status: u16,
    /// Google's own words when it refused, folded and capped
    /// (`talaria_google_errors::quote_raw`), never reworded; None when it
    /// accepted (the result is the caller's mail labels) or sent nothing.
    pub text: Option<String>,
}

/// Call `CHECK_TOOL` on Gmail's MCP server with `token`. `Err` is a call that
/// never got Google's answer (the request or the read of its body failed):
/// nothing Google said, so nothing to record.
pub async fn check_agentic(token: &str) -> Result<CheckAnswer, String> {
    check_agentic_at(GMAIL.endpoint, token).await
}

async fn check_agentic_at(endpoint: &str, token: &str) -> Result<CheckAnswer, String> {
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": CHECK_TOOL, "arguments": {} },
    });
    let res = http()
        .post(endpoint)
        .bearer_auth(token)
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&rpc)
        .timeout(CHECK_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("the call to {endpoint} did not complete ({e})"))?;
    let status = res.status().as_u16();
    let body = res
        .text()
        .await
        .map_err(|e| format!("Google's answer from {endpoint} could not be read ({e})"))?;
    Ok(read_check_answer(status, &body))
}

/// Google's answer to the check, read. Accepted only as HTTP 2xx carrying a
/// JSON-RPC result without `isError`. A refusal quotes, in order of what came
/// back: the result's text content (how these servers refuse), else the
/// error body through `upstream_reason` (a JSON-RPC error's message, or a
/// Google REST error's code and message), else an HTML page's title, else the
/// body folded and capped.
pub fn read_check_answer(status: u16, body: &str) -> CheckAnswer {
    let refused = |text: Option<String>| CheckAnswer {
        ok: false,
        status,
        text,
    };
    let Some(rpc) = parse_mcp_response(body) else {
        return refused(page_words(body));
    };
    if rpc.get("error").is_some() {
        return refused(upstream_reason(body).or_else(|| page_words(body)));
    }
    match rpc.get("result") {
        Some(result) if result.get("isError").and_then(Value::as_bool) == Some(true) => {
            refused(content_text(result).or_else(|| page_words(body)))
        }
        Some(_) if (200..300).contains(&status) => CheckAnswer {
            ok: true,
            status,
            text: None,
        },
        _ => refused(page_words(body)),
    }
}

/// Google's words in a refused answer — a relayed 401 or 403, a failed
/// listing — read the way the check reads them (`read_check_answer`): the
/// result's text content, else a JSON-RPC or REST error's message, else an
/// HTML page's title, else the body folded and capped. None for an empty
/// body.
pub fn refusal_text(status: u16, body: &str) -> Option<String> {
    read_check_answer(status, body).text
}

/// A tool result's text content, joined and quoted.
fn content_text(result: &Value) -> Option<String> {
    let parts: Vec<&str> = result
        .get("content")?
        .as_array()?
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    (!parts.is_empty()).then(|| quote_raw(&parts.join(" ")))
}

/// Words from a body that is not JSON-RPC: an HTML page's `<title>` (Google's
/// error pages carry their status there), else the body folded and capped;
/// None for an empty body.
fn page_words(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let (Some(open), Some(close)) = (lower.find("<title>"), lower.find("</title>")) {
        let start = open + "<title>".len();
        if close > start {
            let title = trimmed[start..close].trim();
            if !title.is_empty() {
                return Some(quote_raw(title));
            }
        }
    }
    Some(quote_raw(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from https://gmailmcp.googleapis.com/mcp/v1 on 2026-11-17,
    // `tools/call list_labels`, no credential and a bogus bearer: HTTP 401.
    const NO_CREDENTIAL: &str = include_str!("../fixtures/list-labels-401-nocred.json");
    const BOGUS_BEARER: &str = include_str!("../fixtures/list-labels-401-bogus.json");
    // The same server, an unknown JSON-RPC method: HTTP 200 with an error.
    const UNKNOWN_METHOD: &str = include_str!("../fixtures/unknown-method-200.json");
    // `/mcp` instead of `/mcp/v1` (the Sheets reference's sample path): 404.
    const WRONG_PATH: &str = include_str!("../fixtures/wrong-path-404.html");

    #[test]
    fn googles_refusals_are_quoted_as_google_wrote_them() {
        assert_eq!(
            read_check_answer(401, BOGUS_BEARER),
            CheckAnswer {
                ok: false,
                status: 401,
                text: Some("Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.".into()),
            }
        );
        let missing = read_check_answer(401, NO_CREDENTIAL);
        assert!(!missing.ok);
        assert!(
            missing.text.as_deref().is_some_and(
                |t| t.starts_with("Request is missing required authentication credential.")
            ),
            "{missing:?}"
        );
        assert_eq!(
            read_check_answer(200, UNKNOWN_METHOD),
            CheckAnswer {
                ok: false,
                status: 200,
                text: Some("Method not supported".into()),
            },
            "an HTTP 200 carrying a JSON-RPC error is no acceptance"
        );
        assert_eq!(
            read_check_answer(404, WRONG_PATH),
            CheckAnswer {
                ok: false,
                status: 404,
                text: Some("Error 404 (Not Found)!!1".into()),
            }
        );
        assert_eq!(
            read_check_answer(502, "  "),
            CheckAnswer {
                ok: false,
                status: 502,
                text: None,
            }
        );
    }

    #[test]
    fn only_a_2xx_result_without_is_error_is_an_acceptance() {
        // MCP's result shape (the spec's, not a captured Google body: no
        // accepted call has been made here). JSON and SSE-framed alike.
        let result =
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"labels"}]}}"#;
        assert_eq!(
            read_check_answer(200, result),
            CheckAnswer {
                ok: true,
                status: 200,
                text: None,
            }
        );
        let sse = format!("event: message\ndata: {result}\n\n");
        assert!(read_check_answer(200, &sse).ok);
        // The same result under a non-2xx status, an explicit isError false,
        // and a refused result under 200.
        assert!(!read_check_answer(403, result).ok);
        assert!(
            read_check_answer(
                200,
                r#"{"jsonrpc":"2.0","id":1,"result":{"content":[],"isError":false}}"#
            )
            .ok
        );
        let refused_200 = read_check_answer(200, BOGUS_BEARER);
        assert!(!refused_200.ok && refused_200.text.is_some());
        // Not JSON-RPC at all, though 200.
        assert!(!read_check_answer(200, "<html><title>OK</title></html>").ok);
        assert!(!read_check_answer(200, r#"{"jsonrpc":"2.0","id":1}"#).ok);
    }

    #[test]
    fn the_catalog_names_each_servers_endpoint_and_the_services_to_enable() {
        assert_eq!(GMAIL.endpoint, "https://gmailmcp.googleapis.com/mcp/v1");
        assert_eq!(PRODUCTS[0], GMAIL);
        let mut ids: Vec<&str> = PRODUCTS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PRODUCTS.len(), "ids are unique");
        for p in PRODUCTS {
            assert!(
                p.endpoint.starts_with("https://")
                    && p.endpoint.ends_with(".googleapis.com/mcp/v1"),
                "{}",
                p.endpoint
            );
            assert!(
                p.mcp_service.ends_with(".googleapis.com"),
                "{}",
                p.mcp_service
            );
            // The endpoint's host is the MCP service.
            assert_eq!(
                p.endpoint
                    .trim_start_matches("https://")
                    .trim_end_matches("/mcp/v1"),
                p.mcp_service
            );
        }
    }
}

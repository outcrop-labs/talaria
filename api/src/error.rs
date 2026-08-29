// Error envelopes. Two shapes live here because the product speaks two:
//
//   house  {"error":"<string>"}            — docs/API-CONVENTIONS.md, everywhere
//   openai {"error":{"message":…}}          — the /api/llm/v1/* exception
//                                            (docs/ARCHITECTURE.md): external
//                                            OpenAI-compatible clients switch on
//                                            these fields and nothing else
//
// Wire structs are typed and in declaration order, ALWAYS. serde_json's map is
// a BTreeMap: a json!-built body would land on the wire alphabetized, which is
// not what JSON.stringify produced and would make byte-diffing a migrated
// route against its TS original impossible. The tests below pin exact bytes.
//
// Consumers land with the models slice (openai_error's 401) and the phase-2
// chat relay (the upstream boundary). DELETE THIS ALLOW as they do.
#![allow(dead_code)]

use axum::{Json, http::StatusCode, response::IntoResponse, response::Response};

#[derive(serde::Serialize)]
pub struct HouseErrorBody {
    pub error: String,
}

pub fn house_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(HouseErrorBody {
            error: message.to_string(),
        }),
    )
        .into_response()
}

#[derive(serde::Serialize)]
pub struct OpenAiErrorBody {
    pub message: String,
    /// Only when an upstream actually sent one — free text never rides, short
    /// provider-chosen tokens do (clients retry on them).
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(serde::Serialize)]
pub struct OpenAiError {
    pub error: OpenAiErrorBody,
}

pub fn openai_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(OpenAiError {
            error: OpenAiErrorBody {
                message: message.to_string(),
                kind: None,
                code: None,
            },
        }),
    )
        .into_response()
}

pub fn openai_error_typed(status: StatusCode, message: &str, kind: &str, code: &str) -> Response {
    (
        status,
        Json(OpenAiError {
            error: OpenAiErrorBody {
                message: message.to_string(),
                kind: Some(kind.to_string()),
                code: Some(code.to_string()),
            },
        }),
    )
        .into_response()
}

// ── Upstream error boundary (port of ui/src/server/upstream-error.ts, #268) ──
// An upstream's error body is written by the upstream — it can name hostnames,
// internals, even the credential we sent. Callers past the proxy get the STATUS
// (ours to share) and a fixed sentence; the verbatim body goes to the log.

pub fn upstream_error_message(status: u16) -> String {
    format!("upstream error ({status})")
}

/// The wire body for a failed upstream hop: fixed sentence plus structured
/// type/code when the upstream sent them, capped at 64 chars each.
pub fn sanitized_upstream_body(status: u16, body: &str) -> String {
    let (kind, code) = structured_tokens(body);
    let error = OpenAiErrorBody {
        message: upstream_error_message(status),
        kind: kind.map(|s| s.chars().take(64).collect()),
        code: code.map(|s| s.chars().take(64).collect()),
    };
    serde_json::to_string(&OpenAiError { error }).expect("typed struct serializes")
}

fn structured_tokens(body: &str) -> (Option<String>, Option<String>) {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let Some(error) = parsed.and_then(|j| j.get("error").cloned()) else {
        return (None, None); // not JSON — an HTML error page or prose
    };
    let s = |v: &serde_json::Value| v.as_str().map(str::to_string);
    (s(&error["type"]), s(&error["code"]))
}

/// The one place the verbatim body is allowed to go, capped at 500 chars.
pub fn log_upstream_error(where_: &str, status: impl std::fmt::Display, body: &str) {
    let cap: String = body.chars().take(500).collect();
    tracing::warn!("[upstream] {where_} {status}: {cap}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn house_envelope_is_exact() {
        let b = serde_json::to_string(&HouseErrorBody {
            error: "not found".into(),
        })
        .unwrap();
        assert_eq!(b, r#"{"error":"not found"}"#);
    }

    #[test]
    fn openai_envelope_drops_absent_fields_and_keeps_order() {
        let b = serde_json::to_string(&OpenAiError {
            error: OpenAiErrorBody {
                message: "invalid API key".into(),
                kind: None,
                code: None,
            },
        })
        .unwrap();
        assert_eq!(b, r#"{"error":{"message":"invalid API key"}}"#);

        let b = serde_json::to_string(&OpenAiError {
            error: OpenAiErrorBody {
                message: "upstream error (429)".into(),
                kind: Some("rate_limit_exceeded".into()),
                code: Some("rate_limited".into()),
            },
        })
        .unwrap();
        // message, type, code — JSON.stringify order, not alphabetical.
        assert_eq!(
            b,
            r#"{"error":{"message":"upstream error (429)","type":"rate_limit_exceeded","code":"rate_limited"}}"#
        );
    }

    #[test]
    fn upstream_boundary_keeps_structured_tokens_and_caps_them() {
        let long_code = "x".repeat(80); // JSON-quoted into the body below
        let body = format!(
            r#"{{"error":{{"message":"secret is hunter2","type":"insufficient_quota","code":"{long_code}"}}}}"#
        );
        let out = sanitized_upstream_body(429, &body);
        assert_eq!(
            out,
            r#"{"error":{"message":"upstream error (429)","type":"insufficient_quota","code":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}}"#
        );
        assert!(
            !out.contains("hunter2"),
            "free text must not ride the boundary"
        );

        // Non-JSON upstream body: fixed sentence only.
        assert_eq!(
            sanitized_upstream_body(502, "<html>Bad Gateway</html>"),
            r#"{"error":{"message":"upstream error (502)"}}"#
        );
    }
}

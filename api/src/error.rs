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
// Consumers: the models slice (openai_error's 401), the chat relay (the null-
// param 429s, the budget facts, the upstream boundary). openai_error_typed
// has no caller yet — the product route groups that emit type+code without a
// null param are a later batch.

/// JS number rendering: JSON.stringify prints whole numbers without a decimal
/// point (1000, not 1000.0) — serde's f64 always writes one. Budget figures
/// ride the wire as numbers, so they get the JS spelling.
pub fn js_num(v: f64) -> serde_json::Value {
    if !v.is_finite() {
        return serde_json::Value::Null;
    }
    if v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
        serde_json::Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    }
}

/// The 429s carry an explicit `param: null` member — the TS literal has it, so
/// the wire does too. `param` is serde_json::Value::Null, which serializes as
/// null; an Option would skip it.
#[derive(serde::Serialize)]
pub struct OpenAiNullParamBody {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub code: String,
    pub param: serde_json::Value,
}

#[derive(serde::Serialize)]
pub struct OpenAiNullParam {
    pub error: OpenAiNullParamBody,
}

pub fn openai_error_null_param(
    status: StatusCode,
    message: &str,
    kind: &str,
    code: &str,
) -> Response {
    (
        status,
        Json(OpenAiNullParam {
            error: OpenAiNullParamBody {
                message: message.to_string(),
                kind: kind.to_string(),
                code: code.to_string(),
                param: serde_json::Value::Null,
            },
        }),
    )
        .into_response()
}

/// The budget denial's facts, nested under error.budget — declaration order is
/// the TS literal's (scope, subject, unit, limit, used, windowHours, via).
#[derive(serde::Serialize)]
pub struct BudgetFacts {
    pub scope: String,
    pub subject: Option<String>,
    pub unit: String,
    pub limit: serde_json::Value,
    pub used: serde_json::Value,
    #[serde(rename = "windowHours")]
    pub window_hours: i64,
    pub via: String,
}

#[derive(serde::Serialize)]
pub struct OpenAiBudgetBody {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub code: String,
    pub param: serde_json::Value,
    pub budget: BudgetFacts,
}

#[derive(serde::Serialize)]
pub struct OpenAiBudget {
    pub error: OpenAiBudgetBody,
}

pub fn openai_budget_error(
    status: StatusCode,
    message: &str,
    kind: &str,
    code: &str,
    budget: BudgetFacts,
) -> Response {
    (
        status,
        Json(OpenAiBudget {
            error: OpenAiBudgetBody {
                message: message.to_string(),
                kind: kind.to_string(),
                code: code.to_string(),
                param: serde_json::Value::Null,
                budget,
            },
        }),
    )
        .into_response()
}

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

/// The house error with a `message` field beside it — agent-auth's migration
/// refusals carry the fix as a second field ({error, message}); the pipe is
/// that exact shape, never a merged sentence.
pub fn house_error_msg(status: StatusCode, error: &str, message: &str) -> Response {
    #[derive(serde::Serialize)]
    struct Body<'a> {
        error: &'a str,
        message: &'a str,
    }
    (status, Json(Body { error, message })).into_response()
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

/// type+code, no param member — reserved for the route groups that use it.
#[allow(dead_code)]
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

    #[test]
    fn null_param_envelope_pins_explicit_null_and_order() {
        let b = serde_json::to_string(&OpenAiNullParam {
            error: OpenAiNullParamBody {
                message: "rate limit exceeded for this key: 60 requests per minute".into(),
                kind: "rate_limit_exceeded".into(),
                code: "rate_limit_exceeded".into(),
                param: serde_json::Value::Null,
            },
        })
        .unwrap();
        assert_eq!(
            b,
            r#"{"error":{"message":"rate limit exceeded for this key: 60 requests per minute","type":"rate_limit_exceeded","code":"rate_limit_exceeded","param":null}}"#
        );
    }

    #[test]
    fn budget_envelope_pins_the_nested_facts_and_js_numbers() {
        let b = serde_json::to_string(&OpenAiBudget {
            error: OpenAiBudgetBody {
                message: "…".into(),
                kind: "budget_exceeded".into(),
                code: "budget_exceeded".into(),
                param: serde_json::Value::Null,
                budget: BudgetFacts {
                    scope: "caller".into(),
                    subject: Some("api:my-key".into()),
                    unit: "usd".into(),
                    limit: js_num(2.0),
                    used: js_num(2.5),
                    window_hours: 24,
                    via: "key".into(),
                },
            },
        })
        .unwrap();
        // 2.0 renders "2" the way JSON.stringify would; 2.5 keeps its point.
        assert_eq!(
            b,
            r#"{"error":{"message":"…","type":"budget_exceeded","code":"budget_exceeded","param":null,"budget":{"scope":"caller","subject":"api:my-key","unit":"usd","limit":2,"used":2.5,"windowHours":24,"via":"key"}}}"#
        );
    }

    #[test]
    fn js_num_renders_whole_numbers_the_js_way() {
        assert_eq!(js_num(1000.0).to_string(), "1000");
        assert_eq!(js_num(2.5).to_string(), "2.5");
        assert_eq!(js_num(0.0).to_string(), "0");
        assert_eq!(js_num(f64::NAN).to_string(), "null");
    }
}

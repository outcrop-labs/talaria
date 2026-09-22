// Error envelopes. Two shapes live here because the product speaks two:
//
//   house  {"error":"<string>"}            — docs/API-CONVENTIONS.md, everywhere
//   openai {"error":{"message":…}}          — the /api/llm/v1/* exception
//                                            (docs/ARCHITECTURE.md): external
//                                            OpenAI-compatible clients switch on
//                                            these fields and nothing else
//
// Wire structs are typed and in declaration order, ALWAYS. serde_json's map is
// a BTreeMap: a json!-built body would land on the wire alphabetized — not
// the field order these envelopes promise. The tests below pin exact bytes.
//
// Consumers: the models slice (openai_error's 401), the chat relay (the null-
// param 429s, the budget facts, the upstream boundary). openai_error_typed
// has no caller yet — reserved for route groups that emit type+code without a
// null param.

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

/// The 429s carry an explicit `param: null` member — the wire spells it
/// explicitly. `param` is serde_json::Value::Null, which serializes as
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

/// The budget denial's facts, nested under error.budget — declaration order
/// is the wire order (scope, subject, unit, limit, used, windowHours, via).
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

/// The house 500 envelope: `Internal Server Error` as text/plain, byte-exact.
/// An unhandled exception (a pg error above all) never reaches a json body —
/// clients of long standing match these bytes, so a house-shaped JSON 500
/// here would be a body they have never seen. Routes that guard a failure
/// with their own catch-and-answer keep house_error and put their own
/// sentence on the wire.
pub fn thrown_internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        "Internal Server Error",
    )
        .into_response()
}

/// Log `context` with the error and answer the house 500 — the one shape a
/// route uses when an engine call it does not catch has failed.
///
/// WHY IT EXISTS: the `tracing::error!("…: {e}"); return thrown_internal_error();`
/// pair was written out by hand at every call site that did not catch an engine
/// error, which made the single decision in it — that an unhandled failure is a
/// LOGGED 500 and never a JSON body — a decision each site re-made, and about a
/// quarter of them re-made by dropping the log. The pair is one concept, so it
/// is one call: the `context` keeps the log line grep-able by domain, the way
/// the hand-written ones were.
pub fn internal(context: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!("{context}: {e}");
    thrown_internal_error()
}

/// The parsed request body as an object, or the house 400 carrying the zod
/// sentence — the shape ~160 route handlers wrote out by hand:
///
/// ```text
/// let obj = match as_object(&parsed) {
///     Ok(o) => o,
///     Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
/// };
/// ```
///
/// WHY IT EXISTS: `talaria_body` deliberately stays pure — it answers the
/// MESSAGE and knows nothing about HTTP — so the message-to-400 conversion
/// lived at every call site instead. It is one conversion, and it belongs
/// beside the envelope it produces, not 160 times inside the routes.
// The Err here IS the response to send — a Response is the only thing a route
// can return, and clippy's size heuristic cannot know that.
#[allow(clippy::result_large_err)]
pub fn object_or_400(
    parsed: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>, Response> {
    match talaria_body::as_object(parsed) {
        Ok(o) => Ok(o),
        Err(msg) => Err(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
}

/// The bare Postgres sentence under a sqlx error — the message
/// catch-and-answer routes put on the wire. sqlx's own Display wraps it
/// ("error returned from database: … at line N"), which the wire never
/// shows.
pub fn pg_message(e: &sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => e.to_string(),
    }
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

// ── Upstream error boundary (#268) ──
// An upstream's error body is written by the upstream — it can name hostnames,
// internals, even the credential we sent. Callers past the proxy get the STATUS
// (ours to share) and a fixed sentence; the verbatim body goes to the log.

pub fn upstream_error_message(status: u16) -> String {
    format!("upstream error ({status})")
}

/// The wire body for a failed upstream hop: OUR sentence — the account's own
/// limit when the body names one, else the fixed one — plus structured
/// type/code when the upstream sent them, capped at 64 chars each.
pub fn sanitized_upstream_body(status: u16, body: &str) -> String {
    let tokens = upstream_tokens(body);
    let error = OpenAiErrorBody {
        message: account_limit(&tokens).unwrap_or_else(|| upstream_error_message(status)),
        kind: cap(tokens.kind),
        code: cap(tokens.code),
    };
    serde_json::to_string(&OpenAiError { error }).expect("typed struct serializes")
}

/// The structured tokens an upstream sent, plus its own message text — which
/// is read, classified, and never forwarded.
struct UpstreamTokens {
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

fn upstream_tokens(body: &str) -> UpstreamTokens {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let error = parsed.as_ref().and_then(|j| j.get("error"));
    let s =
        |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_str).map(str::to_string);
    UpstreamTokens {
        kind: s(error.and_then(|e| e.get("type"))),
        code: s(error.and_then(|e| e.get("code"))),
        message: s(error.and_then(|e| e.get("message"))),
    }
}

/// The 64-char ceiling the boundary puts on a token it forwards.
fn cap(token: Option<String>) -> Option<String> {
    token.map(|t| t.chars().take(64).collect())
}

// ── The provider's own limit, said in our words ──
// A refusal for ACCOUNT reasons is not a bad request: the fix is billing, a
// plan, or waiting for a reset, and "upstream error (400)" sends the reader
// hunting for a malformed payload that does not exist — an agent hit its
// model's monthly ceiling for a week and every turn in every chat read as a
// generic 400. The classification is STRUCTURE ONLY — the OpenAI-compatible
// quota codes, and the one Anthropic limit sentence whose tail is a reset
// instant — because the prose around a provider's message is exactly what
// this boundary exists to keep.

/// Codes that mean "this account is out of quota", as the
/// OpenAI-compatible providers spell it.
const QUOTA_CODES: [&str; 5] = [
    "insufficient_quota",
    "quota_exceeded",
    "usage_limit_reached",
    "billing_hard_limit_reached",
    "credit_balance_too_low",
];

/// Anthropic's monthly-limit sentence, whose tail is the reset instant.
const ANTHROPIC_LIMIT_PREFIX: &str =
    "You have reached your specified API usage limits. You will regain access on ";

/// The account-limit sentence for an upstream body, or None when the body
/// says nothing structured about the account.
fn account_limit(t: &UpstreamTokens) -> Option<String> {
    if let Some(until) = t.message.as_deref().and_then(reset_instant) {
        return Some(format!(
            "the upstream account's usage limit is exhausted — access returns {until}"
        ));
    }
    let named = |token: Option<&str>| token.is_some_and(|t| QUOTA_CODES.contains(&t));
    if named(t.code.as_deref()) || named(t.kind.as_deref()) {
        return Some(
            "the upstream account is out of credit or over its usage limit — check the provider's billing"
                .to_string(),
        );
    }
    None
}

/// The reset instant out of that sentence, accepted only when its tail IS an
/// instant — a date, optionally a clock and a zone — and nothing else. A
/// provider that starts writing anything past the prefix, or appending to a
/// real instant, must not be able to push it across the boundary.
fn reset_instant(message: &str) -> Option<&str> {
    let tail = message.strip_prefix(ANTHROPIC_LIMIT_PREFIX)?;
    let tail = tail.strip_suffix('.').unwrap_or(tail);
    is_instant(tail).then_some(tail)
}

/// An instant as the provider spells it, as TOKENS rather than a whitelist of
/// characters: prose that merely opens with a date has nowhere to hide.
fn is_instant(s: &str) -> bool {
    if !s.is_ascii() || s.len() > 40 {
        return false;
    }
    let mut tokens = s.split_whitespace();
    let Some(date) = tokens.next() else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let mut zone: Option<&str> = None;
    if let Some(next) = tokens.next() {
        if next != "at" {
            return false;
        }
        let Some(clock) = tokens.next() else {
            return false;
        };
        if !is_clock(clock) {
            return false;
        }
        zone = tokens.next();
    }
    if tokens.next().is_some() {
        return false;
    }
    zone.is_none_or(|z| z.len() <= 8 && z.bytes().all(|b| b.is_ascii_uppercase()))
}

fn is_date(s: &str) -> bool {
    let mut parts = s.split('-');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(y), Some(m), Some(d), None) if is_digits(y, 4) && is_digits(m, 2) && is_digits(d, 2)
    )
}

fn is_clock(s: &str) -> bool {
    let mut parts = s.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(m), None, None) => is_digits(h, 2) && is_digits(m, 2),
        (Some(h), Some(m), Some(sec), None) => {
            is_digits(h, 2) && is_digits(m, 2) && is_digits(sec, 2)
        }
        _ => false,
    }
}

fn is_digits(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit())
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
        // message, type, code — declaration order, not alphabetical.
        assert_eq!(
            b,
            r#"{"error":{"message":"upstream error (429)","type":"rate_limit_exceeded","code":"rate_limited"}}"#
        );
    }

    #[test]
    fn upstream_boundary_keeps_structured_tokens_and_caps_them() {
        let long_code = "x".repeat(80); // JSON-quoted into the body below
        let body = format!(
            r#"{{"error":{{"message":"secret is hunter2","type":"rate_limit_exceeded","code":"{long_code}"}}}}"#
        );
        let out = sanitized_upstream_body(429, &body);
        assert_eq!(
            out,
            r#"{"error":{"message":"upstream error (429)","type":"rate_limit_exceeded","code":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}}"#
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
    fn the_account_s_own_limit_is_named_instead_of_the_generic_sentence() {
        // The incident, verbatim: Anthropic's monthly ceiling, which used to
        // reach every chat as "upstream error (400)". The reset instant rides
        // (it is what makes the sentence actionable); the provider's prose
        // does not.
        let body = r#"{"error":{"code":"invalid_request_error","message":"You have reached your specified API usage limits. You will regain access on 2026-10-01 at 00:00 UTC.","type":"invalid_request_error","param":null}}"#;
        assert_eq!(
            sanitized_upstream_body(400, body),
            r#"{"error":{"message":"the upstream account's usage limit is exhausted — access returns 2026-10-01 at 00:00 UTC","type":"invalid_request_error","code":"invalid_request_error"}}"#
        );

        // The OpenAI-compatible spelling: the code alone is the signal.
        assert_eq!(
            sanitized_upstream_body(
                429,
                r#"{"error":{"message":"You exceeded your current quota, please check your plan and billing details.","type":"insufficient_quota","code":"insufficient_quota"}}"#
            ),
            r#"{"error":{"message":"the upstream account is out of credit or over its usage limit — check the provider's billing","type":"insufficient_quota","code":"insufficient_quota"}}"#
        );
    }

    #[test]
    fn a_look_alike_limit_sentence_stays_behind_the_boundary() {
        // Same opening, prose tail: not an instant, so nothing crosses.
        let prose = r#"{"error":{"type":"invalid_request_error","message":"You have reached your specified API usage limits. You will regain access on the moon, ask ops at https://internal.host."}}"#;
        assert_eq!(
            sanitized_upstream_body(400, prose),
            r#"{"error":{"message":"upstream error (400)","type":"invalid_request_error"}}"#
        );

        // A real instant with anything appended after it is still a look-alike.
        let appended = r#"{"error":{"type":"invalid_request_error","message":"You have reached your specified API usage limits. You will regain access on 2026-10-01 at 00:00 UTC. ping https://internal.host with sk-abc"}}"#;
        let out = sanitized_upstream_body(400, appended);
        assert_eq!(
            out,
            r#"{"error":{"message":"upstream error (400)","type":"invalid_request_error"}}"#
        );
        assert!(!out.contains("internal.host") && !out.contains("sk-abc"));
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
        // 2.0 renders "2"; 2.5 keeps its point.
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

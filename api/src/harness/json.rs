// The ONE structured-output parser — the port of harness/json.ts. Every
// place Talaria asks a model for JSON and reads it back comes through here.
//
// WHY THIS MODULE EXISTS
//   Six different extractors grew up in the TS codebase, and three of them
//   were the same non-solution: take everything from the first `{` to the
//   last `}` and hand it to JSON.parse. That is not a JSON scanner. It is a
//   substring, and it was verified by EXECUTION to fail on three shapes a
//   14B model emits constantly:
//
//     1. a fenced object followed by prose that mentions a `{placeholder}`
//     2. a preamble, then two objects (the model "shows its work", then answers)
//     3. an object, then a bulleted explanation containing a brace
//
//   In all three the greedy span swallows the trailing prose and the parse
//   dies. A brace-BALANCING scan that knows what a string literal is handles
//   all three and costs one pass.
//
//   The failure mattered more than it looks, because each of the six sites
//   answers a failed parse DIFFERENTLY and silently: the judge escalates to a
//   human, the blurb writer returns 0 and re-burns the same batch forever,
//   Muse returns null so the button just does nothing. One extractor, one
//   parse result, and the caller declares what a failure means.
//
// WHAT THIS MODULE IS NOT
//   It is not a JSON5 parser and it does not guess at truncation. A value
//   whose braces never close is a FAILURE, reported as one, with a repair
//   instruction the caller can hand straight back to the model. Inventing
//   the missing tail is how a harness returns a confidently wrong answer
//   instead of an error.
//
//   Pure by construction: no DB, no gateway, no settings.
//
// THE ONE RECORDED DIVERGENCE: the parenthetical in "the JSON could not be
// parsed (…)" is the STRICT parser's own error text, which on the TS side is
// the runtime's prose — bun says "JSON Parse error: Unexpected token '}'",
// node says something else again, and serde_json says a third thing. The
// sentence's SHAPE (a short parse complaint inside the fixed prefix) is the
// contract a model reads; the prose inside the parens was never stable
// enough to be one, so serde_json's wording is accepted and the TS corpus
// does not pin it.

use super::schema::{self, Schema};
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

// ── Scanning ─────────────────────────────────────────────────────────────────

/// Where the value opened at `start` closes, or None if it never does.
///
/// String-literal and escape aware, which is the entire point:
/// `{"note": "}"}` and `{"q": "he said \"hi\""}` are one complete value
/// each, and every "first brace to last brace" extractor gets both wrong.
///
/// Depth is counted across both bracket families rather than kept on a
/// stack. A crossed pair (`{"a": 1]`) therefore reads as a complete-but-bogus
/// candidate; the parse adjudicates it a moment later and the scan moves on
/// to the next candidate, which is the behaviour we want anyway.
fn balanced_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth: i64 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        let ch = char::from(b);
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
        } else if ch == '{' || ch == '[' {
            depth += 1;
        } else if ch == '}' || ch == ']' {
            depth -= 1;
            if depth <= 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Every complete `{`/`[`-rooted span in `text`, outermost first, in order.
///
/// Stops dead at the first opener that never closes. That looks lossy —
/// there could be a good object further down — but it is the truncation
/// guarantee: once an opener is unterminated, everything after it is INSIDE
/// an unfinished value, so anything found there would be a fragment
/// presented as an answer. A cut-off response has to fail.
fn balanced_spans(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if ch != '{' && ch != '[' {
            i += 1;
            continue;
        }
        match balanced_end(text, i) {
            None => return spans,
            Some(end) => {
                spans.push(&text[i..end + 1]);
                i = end + 1;
            }
        }
    }
    spans
}

fn fence_block_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?:^|\n)[ \t]*(?:`{3,}|~{3,})[^\n]*\n([\s\S]*?)(?:\n[ \t]*(?:`{3,}|~{3,})|$)")
            .unwrap()
    })
}

/// Contents of every ``` / ~~~ fenced block, in order. An unclosed fence
/// runs to the end of the text — models drop the closing fence constantly,
/// usually on exactly the responses that were already near the token cap.
fn fenced_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for caps in fence_block_re().captures_iter(text) {
        if let Some(body) = caps.get(1).filter(|b| !b.as_str().is_empty()) {
            blocks.push(body.as_str().to_string());
        }
    }
    blocks
}

fn fence_line_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?m)^[ \t]*(?:`{3,}|~{3,})[^\n]*$").unwrap())
}

/// Fence delimiter lines removed, so a fenced value scans as ordinary text.
/// A line that is only a fence marker is never part of a JSON value — JSON
/// has no raw newlines inside strings, so this cannot cut into one.
fn strip_fences(text: &str) -> String {
    fence_line_re().replace_all(text, "").into_owned()
}

/// Candidate JSON spans, best-guess first, as owned strings (fenced bodies
/// are owned; prose spans are subslices of a temporary).
///
/// Fenced blocks come BEFORE the surrounding prose because a fence is the
/// model explicitly saying "this is the payload". Without that ordering,
/// "According to [1], here is the list: ```[\"a\",\"b\"]```" extracts `[1]` —
/// a perfectly valid JSON array that happens to be a citation marker. Order
/// by intent first, position second.
fn candidates(text: &str) -> Vec<String> {
    fn fresh<'a, I: Iterator<Item = &'a str>>(
        spans: I,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        for span in spans {
            if seen.insert(span.to_string()) {
                out.push(span.to_string());
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for block in fenced_blocks(text) {
        fresh(balanced_spans(&block).into_iter(), &mut seen, &mut out);
    }
    let stripped = strip_fences(text);
    fresh(balanced_spans(&stripped).into_iter(), &mut seen, &mut out);
    out
}

/// First complete, balanced JSON value in the text (`{` or `[` rooted), or
/// None.
///
/// String-literal and escape aware, so a brace inside a string never closes
/// the value. Markdown fences are stripped first, and fenced content is
/// preferred over prose. If a candidate does not parse — a `{placeholder}`
/// in an explanation, say — the scan continues to LATER candidates rather
/// than giving up, which is what turns "the model rambled first" from a
/// failure into a non-event.
///
/// Viability is judged by parsing the span OR of `relax_json(span)`, so a
/// trailing comma in the real answer never causes the scanner to walk past
/// it and return some decorative brace group from the prose instead.
///
/// Returns the span EXACTLY as the model wrote it, not the relaxed rewrite:
/// callers that log a failure should log what actually came back.
pub fn extract_json(text: &str) -> Option<String> {
    candidates(text)
        .into_iter()
        .find(|span| try_parse(span).is_ok())
}

// ── Relaxation ───────────────────────────────────────────────────────────────

const CURLY_DOUBLE: [char; 2] = ['“', '”'];
const CURLY_SINGLE: [char; 2] = ['‘', '’'];
/// Non-space characters that can only precede an OPENING string delimiter.
const BEFORE_OPEN: [char; 4] = ['{', '[', ':', ','];
/// Non-space characters that can only follow a CLOSING string delimiter.
const AFTER_CLOSE: [char; 4] = ['}', ']', ':', ','];

/// Bare literals JSON has no word for, mapped to null. `undefined` is in
/// the list because a model imitating JS emits it about as often as NaN.
/// LONGEST FIRST: `-Infinity` must win over `Infinity` when both match.
const BARE_LITERALS: [&str; 5] = ["-Infinity", "+Infinity", "Infinity", "NaN", "undefined"];

fn next_non_space(s: &[char], from: usize) -> Option<char> {
    s.iter().skip(from).find(|c| !c.is_whitespace()).copied()
}

fn prev_non_space(s: &[char], before: usize) -> Option<char> {
    s.iter().take(before).rfind(|c| !c.is_whitespace()).copied()
}

/// Curly quotes sitting where JSON demands a delimiter become straight
/// quotes.
///
/// Position, not identity, is the test — a model that types
/// `{“verdict”: “pass”}` puts its curly quotes exactly where `"` belongs,
/// while the apostrophe in `"the model’s answer"` has letters on both sides
/// and is left alone. A blanket replace would corrupt prose content; this
/// only touches quotes that are structurally quotes.
fn straighten_quotes(raw: &[char]) -> String {
    let mut out = String::with_capacity(raw.len());
    for (i, &ch) in raw.iter().enumerate() {
        if !CURLY_DOUBLE.contains(&ch) && !CURLY_SINGLE.contains(&ch) {
            out.push(ch);
            continue;
        }
        let before = prev_non_space(raw, i);
        let after = next_non_space(raw, i + 1);
        let structural = match (before, after) {
            (None, _) | (_, None) => true,
            (Some(b), Some(a)) => BEFORE_OPEN.contains(&b) || AFTER_CLOSE.contains(&a),
        };
        out.push(if structural { '"' } else { ch });
    }
    out
}

/// JS `/[\w$]/` — ASCII word characters plus the dollar sign.
fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
}

/// Tolerant repair of the shapes small models actually emit:
///   - a trailing comma before `}` or `]`
///   - curly quotes used as string delimiters
///   - bare NaN / Infinity / undefined, which JSON has no literal for
///   - a raw newline or tab inside a string value
///
/// The last one is safe rather than optimistic: a control character inside
/// a string is ALWAYS invalid JSON, and by the time this runs the extractor
/// has already proved the value's strings are balanced, so escaping it can
/// only be a repair. Everything here is string-literal aware, so a `,]` or
/// the word Infinity inside a string value survives untouched.
///
/// Deliberately NOT attempted: truncation. A value the model never finished
/// is a failure with a repair prompt attached, not a shape to guess at.
/// Also not attempted: single-quoted strings and // comments — both need
/// real disambiguation (apostrophes, URLs) and neither shows up often
/// enough to justify the ambiguity.
pub fn relax_json(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let src = straighten_quotes(&chars);
    let src: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < src.len() {
        let ch = src[i];
        if in_string {
            if escaped {
                escaped = false;
                out.push(ch);
            } else if ch == '\\' {
                escaped = true;
                out.push(ch);
            } else if ch == '"' {
                in_string = false;
                out.push(ch);
            } else if ch == '\n' {
                out.push_str("\\n");
            } else if ch == '\r' {
                out.push_str("\\r");
            } else if ch == '\t' {
                out.push_str("\\t");
            } else {
                out.push(ch);
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            i += 1;
            continue;
        }
        if ch == ',' {
            let next = next_non_space(&src, i + 1);
            if next == Some('}') || next == Some(']') {
                i += 1;
                continue;
            }
            out.push(ch);
            i += 1;
            continue;
        }
        // A bare literal only counts where a word boundary brackets it —
        // "to Infinity and beyond" inside a string is untouched (handled by
        // the in_string arm), and `InfinityMode` outside one is an
        // identifier, not a literal.
        // `String.startsWith` answers false when the candidate runs off the
        // end; a slice here would panic — so bound it first.
        let literal = BARE_LITERALS.iter().find(|lit| {
            let end = i + lit.chars().count();
            end <= src.len()
                && src[i..end].iter().collect::<String>() == **lit
                && src.get(end).is_none_or(|c| !is_word_char(*c))
                && (i == 0 || !is_word_char(src[i - 1]))
        });
        if let Some(lit) = literal {
            out.push_str("null");
            i += lit.chars().count();
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

// ── Parsing ──────────────────────────────────────────────────────────────────

pub enum ParseResult {
    Ok(Value),
    Err { error: String, raw: Option<String> },
}

impl ParseResult {
    pub fn ok(&self) -> bool {
        matches!(self, ParseResult::Ok(_))
    }
}

/// serde_json::from_str, then one relaxed retry. Two attempts, never more:
/// if the value survives neither, the model has to be asked again — that is
/// what `repair_prompt` is for, and it works far better than a third
/// heuristic. The error string is the STRICT attempt's prose (see the module
/// header for the recorded divergence).
fn try_parse(span: &str) -> Result<Value, String> {
    match serde_json::from_str(span) {
        Ok(v) => Ok(v),
        Err(strict) => match serde_json::from_str::<Value>(&relax_json(span)) {
            Ok(v) => Ok(v),
            Err(_) => Err(strict.to_string()),
        },
    }
}

/// At most three problems. A small model handed a list of eleven fixes tends
/// to rewrite the whole thing from scratch and reintroduce the first one.
fn describe_issues(issues: &[(Vec<schema::Seg>, schema::Issue)], root: &Value) -> String {
    let named: Vec<String> = issues
        .iter()
        .take(3)
        .map(|i| schema::describe_issue(i, root))
        .collect();
    if named.is_empty() {
        "the value did not match the required shape".into()
    } else {
        named.join("; ")
    }
}

/// Extract, parse (with one relax retry), then validate against `schema`.
///
/// Candidates are tried in order and the FIRST one that both parses and
/// validates wins, so a model that narrates before answering, or drops a
/// `[1]` citation marker ahead of its real array, still gets read correctly.
/// When nothing validates, the error describes the first candidate that at
/// least parsed — the model's actual attempt, not some brace group in its
/// preamble.
///
/// `error` is written to be fed BACK TO THE MODEL verbatim (see
/// `repair_prompt`): it names the concrete problem — "expected object, got
/// array", "missing required field 'verdict'" — and never carries a stack
/// trace, a schema dump, or an internal type name. `raw` is what was
/// extracted, for logs, and is None when nothing complete was found at all.
pub fn parse_json(text: &str, schema: &Schema) -> ParseResult {
    let mut first_valid: Option<(String, String)> = None; // (error, raw)
    let mut first_broken: Option<(String, String)> = None; // (reason, raw)

    for span in candidates(text) {
        let parsed = match try_parse(&span) {
            Ok(v) => v,
            Err(reason) => {
                if first_broken.is_none() {
                    first_broken = Some((reason, span));
                }
                continue;
            }
        };
        let (value, issues) = schema::validate(schema, &parsed);
        if issues.is_empty() {
            return ParseResult::Ok(value);
        }
        if first_valid.is_none() {
            let error = describe_issues(&issues, &parsed);
            first_valid = Some((error, span));
        }
    }

    if let Some((error, raw)) = first_valid {
        return ParseResult::Err {
            error,
            raw: Some(raw),
        };
    }
    if let Some((reason, raw)) = first_broken {
        return ParseResult::Err {
            error: format!("the JSON could not be parsed ({reason})"),
            raw: Some(raw),
        };
    }

    // Nothing complete was found. Distinguish "started a value and never
    // finished it" from "answered in prose", because those are opposite
    // instructions: one model needs to be told it was cut off, the other
    // needs to be told to stop talking. Guessing at the missing tail is not
    // on the menu — see relax_json.
    let truncated = strip_fences(text).contains(['{', '[']);
    ParseResult::Err {
        error: if truncated {
            "the JSON value was opened but never closed - the response looks truncated".into()
        } else {
            "no JSON object or array was found in the response".into()
        },
        raw: None,
    }
}

/// The repair turn's user message. One place, so every harness repairs
/// identically and a change to the wording is measurable across all of them
/// at once. Short and imperative on purpose: this text is spent on the
/// models least able to afford it, and "no prose" has to be the last thing
/// read.
pub fn repair_prompt(error: &str) -> String {
    format!(
        "That response could not be used: {error}.\n\nSend the corrected JSON value only - no explanation before or after it, no markdown code fence."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Fenced object, then prose that mentions a brace. The single most
    /// common small-model response shape there is.
    const FENCED_THEN_PROSE: &str = "Here is the verdict:\n\n```json\n{\"verdict\": \"pass\", \"summary\": \"The fix matches the ticket.\"}\n```\n\nNote: the {summary} field is deliberately short.";

    /// A preamble, then the model shows its work and answers twice.
    const PREAMBLE_TWO_OBJECTS: &str = "Let me think about this. My first read:\n{\"verdict\": \"pass\", \"summary\": \"Looks complete.\"}\nOn reflection I would also accept:\n{\"verdict\": \"revise\", \"summary\": \"Missing a test.\"}";

    /// The answer, then a bulleted explanation containing a brace.
    const OBJECT_THEN_BULLETS: &str = "{\"verdict\": \"revise\", \"summary\": \"No test covers the new branch.\"}\n\n- I checked the diff\n- The {issues} array is empty because nothing else stood out";

    // The two strategies this module replaces (greedy regex,
    // indexOf/lastIndexOf) are transcribed so "these used to fail" is an
    // assertion, not a claim.
    fn legacy_greedy(text: &str) -> Option<Value> {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end <= start {
            return None;
        }
        serde_json::from_str(&text[start..end + 1]).ok()
    }
    fn legacy_index_of(text: &str) -> Option<Value> {
        legacy_greedy(text)
    }

    fn verdict_schema() -> Schema {
        Schema::Object(vec![
            schema::Field::required(
                "verdict",
                Schema::Enum(
                    ["pass", "revise", "escalate"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
            ),
            schema::Field::required("summary", Schema::string()),
            schema::Field::required(
                "issues",
                Schema::with_default(Schema::Array(Box::new(Schema::string())), json!([])),
            ),
        ])
    }

    // ── extract_json ───────────────────────────────────────────────────────

    #[test]
    fn reads_a_fenced_object_followed_by_prose_containing_a_brace() {
        assert_eq!(
            extract_json(FENCED_THEN_PROSE),
            Some("{\"verdict\": \"pass\", \"summary\": \"The fix matches the ticket.\"}".into())
        );
    }

    #[test]
    fn takes_the_first_object_when_a_preamble_is_followed_by_two_of_them() {
        assert_eq!(
            extract_json(PREAMBLE_TWO_OBJECTS),
            Some("{\"verdict\": \"pass\", \"summary\": \"Looks complete.\"}".into())
        );
    }

    #[test]
    fn reads_an_object_followed_by_bulleted_prose_containing_a_brace() {
        assert_eq!(
            extract_json(OBJECT_THEN_BULLETS),
            Some(
                "{\"verdict\": \"revise\", \"summary\": \"No test covers the new branch.\"}".into()
            )
        );
    }

    #[test]
    fn the_legacy_extractors_fail_where_this_one_succeeds() {
        for text in [FENCED_THEN_PROSE, PREAMBLE_TWO_OBJECTS, OBJECT_THEN_BULLETS] {
            assert!(legacy_greedy(text).is_none());
            assert!(legacy_index_of(text).is_none());
            assert!(extract_json(text).is_some());
        }
        // …and both legacy strategies did work on a clean object — which is
        // why they shipped.
        assert_eq!(legacy_greedy("{\"a\": 1}"), Some(json!({"a": 1})));
    }

    #[test]
    fn does_not_let_a_brace_inside_a_string_literal_close_the_value() {
        let text = "Sure: {\"note\": \"use {curly} braces here\", \"ok\": true} — hope that helps.";
        assert_eq!(
            extract_json(text),
            Some("{\"note\": \"use {curly} braces here\", \"ok\": true}".into())
        );
    }

    #[test]
    fn does_not_let_an_escaped_quote_inside_a_string_literal_end_it() {
        let text = "{\"q\": \"he said \\\"go}\\\" and left\", \"n\": 2}";
        let span = extract_json(text).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&span).unwrap(),
            json!({"q": "he said \"go}\" and left", "n": 2})
        );
    }

    #[test]
    fn handles_an_array_root() {
        assert_eq!(
            extract_json("Queries:\n[\"one\", \"two\"]\nThat is all."),
            Some("[\"one\", \"two\"]".into())
        );
    }

    #[test]
    fn handles_nesting_several_levels_deep() {
        let value = json!({"plan": {"steps": [{"title": "a", "tags": ["x", "y"]}]}});
        let rendered = serde_json::to_string(&value).unwrap();
        assert_eq!(
            extract_json(&format!("prefix {rendered} suffix")),
            Some(rendered)
        );
    }

    #[test]
    fn walks_past_a_prose_brace_group_that_is_not_json() {
        let text = "Replace {placeholder} with the value, like so: {\"name\": \"talaria\"}";
        assert_eq!(extract_json(text), Some("{\"name\": \"talaria\"}".into()));
    }

    #[test]
    fn prefers_a_fenced_block_over_an_earlier_citation_marker() {
        let text = "According to [1] the answer is:\n```json\n[\"alpha\", \"beta\"]\n```";
        assert_eq!(extract_json(text), Some("[\"alpha\", \"beta\"]".into()));
    }

    #[test]
    fn handles_a_fenced_block_whose_closing_fence_the_model_forgot() {
        assert_eq!(
            extract_json("```json\n{\"a\": 1}"),
            Some("{\"a\": 1}".into())
        );
    }

    #[test]
    fn handles_tilde_fences() {
        assert_eq!(
            extract_json("~~~\n{\"a\": 1}\n~~~"),
            Some("{\"a\": 1}".into())
        );
    }

    #[test]
    fn returns_the_span_verbatim_not_the_relaxed_rewrite() {
        assert_eq!(extract_json("{\"a\": 1,}"), Some("{\"a\": 1,}".into()));
    }

    #[test]
    fn returns_none_on_prose_with_no_json_in_it_at_all() {
        assert_eq!(
            extract_json("I am not sure I can answer that, sorry."),
            None
        );
    }

    #[test]
    fn returns_none_on_a_truncated_value_rather_than_guessing_at_the_tail() {
        assert_eq!(
            extract_json("{\"verdict\": \"pass\", \"summary\": \"the fix looks"),
            None
        );
        assert_eq!(extract_json("[\"one\", \"two\", \"thr"), None);
        // The inner object is complete and parseable. Returning it would be
        // a fragment presented as the answer, which is exactly the failure
        // mode.
        assert_eq!(extract_json("{\"outer\": 1, \"inner\": {\"c\": 2}"), None);
    }

    // ── relax_json ─────────────────────────────────────────────────────────

    fn relaxed(raw: &str) -> Value {
        serde_json::from_str(&relax_json(raw)).unwrap()
    }

    #[test]
    fn drops_trailing_commas_in_nested_arrays_and_objects() {
        assert_eq!(
            relaxed("{\"tags\": [\"x\", \"y\",], \"meta\": {\"n\": 1,},}"),
            json!({"tags": ["x", "y"], "meta": {"n": 1}})
        );
    }

    #[test]
    fn straightens_curly_quotes_used_as_delimiters_around_keys_and_values() {
        assert_eq!(relaxed("{“verdict”: “pass”}"), json!({"verdict": "pass"}));
    }

    #[test]
    fn leaves_a_curly_apostrophe_inside_a_string_value_alone() {
        assert_eq!(
            relaxed("{\"a\": \"the model’s answer\", \"b\": 1,}"),
            json!({"a": "the model’s answer", "b": 1})
        );
    }

    #[test]
    fn leaves_a_trailing_comma_lookalike_inside_a_string_value_alone() {
        assert_eq!(
            relaxed("{\"a\": \"x,]\", \"b\": 1,}"),
            json!({"a": "x,]", "b": 1})
        );
    }

    #[test]
    fn maps_bare_nan_infinity_and_undefined_to_null() {
        assert_eq!(
            relaxed("{\"a\": NaN, \"b\": Infinity, \"c\": -Infinity, \"d\": undefined}"),
            json!({"a": null, "b": null, "c": null, "d": null})
        );
    }

    #[test]
    fn leaves_the_word_infinity_inside_a_string_value_alone() {
        assert_eq!(
            relaxed("{\"a\": \"to Infinity and beyond\", \"b\": NaN}"),
            json!({"a": "to Infinity and beyond", "b": null})
        );
    }

    #[test]
    fn escapes_a_raw_newline_inside_a_string_value() {
        assert_eq!(
            relaxed("{\"body\": \"line one\nline two\"}"),
            json!({"body": "line one\nline two"})
        );
    }

    #[test]
    fn leaves_already_valid_json_byte_identical() {
        let valid = "{\"a\": [1, 2], \"b\": {\"c\": \"d\"}}";
        assert_eq!(relax_json(valid), valid);
    }

    #[test]
    fn does_not_invent_a_closing_brace_for_a_truncated_value() {
        assert!(serde_json::from_str::<Value>(&relax_json("{\"a\": 1")).is_err());
    }

    // ── parse_json ─────────────────────────────────────────────────────────

    #[test]
    fn validates_the_extracted_value_against_the_schema() {
        let result = parse_json(FENCED_THEN_PROSE, &verdict_schema());
        match result {
            ParseResult::Ok(v) => assert_eq!(
                v,
                json!({"verdict": "pass", "summary": "The fix matches the ticket.", "issues": []})
            ),
            ParseResult::Err { error, .. } => panic!("expected ok, got {error}"),
        }
    }

    #[test]
    fn recovers_a_trailing_comma_through_the_relax_retry() {
        assert!(
            parse_json(
                "{\"verdict\": \"pass\", \"summary\": \"ok\", \"issues\": [],}",
                &verdict_schema()
            )
            .ok()
        );
    }

    #[test]
    fn names_the_missing_field_and_carries_the_raw() {
        let result = parse_json("{\"summary\": \"ok\"}", &verdict_schema());
        match result {
            ParseResult::Ok(_) => panic!("expected failure"),
            ParseResult::Err { error, raw } => {
                assert!(
                    error.contains("missing required field 'verdict'"),
                    "{error}"
                );
                assert_eq!(raw, Some("{\"summary\": \"ok\"}".into()));
            }
        }
    }

    #[test]
    fn names_a_nested_missing_field_by_path() {
        let schema = Schema::Object(vec![schema::Field::required(
            "plan",
            Schema::Object(vec![schema::Field::required("title", Schema::string())]),
        )]);
        match parse_json("{\"plan\": {}}", &schema) {
            ParseResult::Err { error, .. } => assert!(
                error.contains("missing required field 'plan.title'"),
                "{error}"
            ),
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn names_the_field_and_both_types_on_a_type_mismatch() {
        match parse_json(
            "{\"verdict\": \"pass\", \"summary\": 42}",
            &verdict_schema(),
        ) {
            ParseResult::Err { error, .. } => assert!(
                error.contains("field 'summary' should be string, got number"),
                "{error}"
            ),
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn names_the_offending_array_element_by_index() {
        match parse_json(
            "{\"verdict\": \"pass\", \"summary\": \"ok\", \"issues\": [\"a\", 7]}",
            &verdict_schema(),
        ) {
            ParseResult::Err { error, .. } => assert!(
                error.contains("field 'issues[1]' should be string, got number"),
                "{error}"
            ),
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn says_expected_object_got_array_when_the_root_shape_is_wrong() {
        match parse_json(
            "[{\"verdict\": \"pass\", \"summary\": \"ok\"}]",
            &verdict_schema(),
        ) {
            ParseResult::Err { error, .. } => {
                assert!(error.contains("expected object, got array"), "{error}")
            }
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn lists_the_allowed_options_when_an_enum_value_is_wrong() {
        match parse_json(
            "{\"verdict\": \"maybe\", \"summary\": \"ok\"}",
            &verdict_schema(),
        ) {
            ParseResult::Err { error, .. } => {
                assert!(error.contains("field 'verdict' must be one of"), "{error}");
                assert!(error.contains("\"revise\""), "{error}");
            }
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn skips_a_parseable_but_wrong_shaped_candidate_for_a_later_valid_one() {
        // "[1]" is a valid JSON array and a citation marker. The schema is
        // what tells them apart, which is why validation drives the walk.
        let text = "According to [1], the queries are:\n[\"alpha\", \"beta\"]";
        let schema = Schema::Array(Box::new(Schema::string()));
        match parse_json(text, &schema) {
            ParseResult::Ok(v) => assert_eq!(v, json!(["alpha", "beta"])),
            ParseResult::Err { error, .. } => panic!("expected ok, got {error}"),
        }
    }

    #[test]
    fn reports_truncation_as_truncation_with_no_extracted_raw() {
        match parse_json(
            "{\"verdict\": \"pass\", \"summary\": \"the fix looks",
            &verdict_schema(),
        ) {
            ParseResult::Err { error, raw } => {
                assert_eq!(
                    error,
                    "the JSON value was opened but never closed - the response looks truncated"
                );
                assert_eq!(raw, None);
            }
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn reports_prose_with_no_json_at_all_with_no_extracted_raw() {
        match parse_json("I could not safely determine that.", &verdict_schema()) {
            ParseResult::Err { error, raw } => {
                assert_eq!(error, "no JSON object or array was found in the response");
                assert_eq!(raw, None);
            }
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn caps_the_error_at_three_problems() {
        let schema = Schema::Object(vec![
            schema::Field::required("a", Schema::string()),
            schema::Field::required("b", Schema::string()),
            schema::Field::required("c", Schema::string()),
            schema::Field::required("d", Schema::string()),
            schema::Field::required("e", Schema::string()),
        ]);
        match parse_json("{}", &schema) {
            ParseResult::Err { error, .. } => assert_eq!(error.split(";").count(), 3),
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    #[test]
    fn never_leaks_a_stack_trace_or_an_internal_type_name_into_the_error() {
        match parse_json(
            "{\"verdict\": \"pass\", \"summary\": 42}",
            &verdict_schema(),
        ) {
            ParseResult::Err { error, .. } => {
                assert!(!error.contains("ZodError"));
                assert!(!error.contains("Invalid input"), "{error}");
                assert!(!error.contains(" at "), "{error}");
            }
            ParseResult::Ok(_) => panic!("expected failure"),
        }
    }

    // ── repair_prompt ──────────────────────────────────────────────────────

    #[test]
    fn carries_the_concrete_error_and_forbids_everything_but_the_value() {
        let prompt = repair_prompt("missing required field 'verdict'");
        assert!(prompt.contains("missing required field 'verdict'"));
        assert!(prompt.contains("JSON value only"));
        assert!(prompt.contains("no markdown code fence"));
    }
}

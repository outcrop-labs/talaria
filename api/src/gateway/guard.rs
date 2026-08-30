// Confab guardrail — port of ui/src/server/guardrails.ts, reduced to the two
// doors that have crossed: `guard_completion` (the gateway route's config →
// rules → findings → caveat, recording as it goes) and `guard_text` (the
// gate-safe rules over plain text, which agent-writes and the judge pre-pass
// call), plus `redact_secrets` for the strict rewrite and the pieces they
// lean on. A cheap, model-agnostic STRUCTURAL check on model output: no LLM
// call, no added model tokens — regex over the answer plus the turn's tool
// record derived from the request messages.
//
// The checks (first three from the Hermes confab-guard plugin):
//   zero_tool_claim   — claims a completed action, but no external tool ran
//   ungrounded_ref    — cites a link/UUID that wasn't in any tool result
//   fabricated_outage — claims an outage, but no tool actually errored
//   secret_leak       — a live credential shape in the output
//   pii_leak          — high-precision personal data (SSN / Luhn card / IBAN)
//
// GROUNDING, the part worth getting exactly right: a detector matches a SHAPE,
// and business identifiers share shapes with the things the rules are named
// after. If the span also appears in the turn's INPUT, the user handed it to
// the model — it is not evidence about the model, and (per rule) possibly not
// something to rewrite either. The two halves of that decision are declared
// per rule as Groundable, and `evaluate` is the only place that reads it.
//
// JS regex parity: \b and \d are ASCII in JS; the regex crate's are Unicode.
// Every ported pattern substitutes (?-u:\b) / (?-u:\d) the way the vault does.
// The two JS LOOKAHEADS (SSN group exclusions, FUTURE's negation exceptions)
// are hand-checked in code — the regex crate has no lookaround. Sentence
// splitting (a JS lookbehind) is hand-rolled the same way.

use crate::gateway::settings::get_setting;
use crate::gateway::vault;
use regex::Regex;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::OnceLock;

const B: &str = r"(?-u:\b)";
const D: &str = r"(?-u:\d)";

/// JS regex source with ASCII semantics restored. `(?i)` where TS used the
/// flag; the substitution is textual, exactly the vault's trick.
fn js(src: &str, flags: &str) -> Regex {
    let pat = format!("{flags}{}", src.replace("\\b", B).replace("\\d", D));
    Regex::new(&pat).unwrap_or_else(|e| panic!("guard pattern must compile: {e} ({pat})"))
}

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardMode {
    Off,
    Observe,
    Annotate,
    Strict,
}

impl GuardMode {
    /// Only annotate/strict disclose to the reader (the caveat).
    pub fn discloses(self) -> bool {
        matches!(self, GuardMode::Annotate | GuardMode::Strict)
    }
}

#[derive(Debug, Clone)]
pub struct GuardConfig {
    pub mode: GuardMode,
    /// Per-rule on/off keyed by rule id; absent = the rule's default.
    pub checks: serde_json::Map<String, Value>,
    pub min_confidence: f64,
    pub policed_hosts: Vec<String>,
}

/// Port of getGuardConfig's `{...DEFAULT_CONFIG, ...stored}` — off modes and
/// garbage values fall back to the default, field by field.
pub async fn guard_config(pg: &PgPool) -> GuardConfig {
    let raw = get_setting(pg, "guardrails_config", serde_json::json!({})).await;
    let mut c = GuardConfig {
        mode: GuardMode::Observe,
        checks: serde_json::Map::new(),
        min_confidence: 0.5,
        policed_hosts: Vec::new(),
    };
    if !raw.is_object() {
        return c;
    }
    c.mode = match raw.get("mode").and_then(|m| m.as_str()) {
        Some("off") => GuardMode::Off,
        Some("annotate") => GuardMode::Annotate,
        Some("strict") => GuardMode::Strict,
        // observe and anything unrecognized: detect + record, tell no one.
        _ => GuardMode::Observe,
    };
    if let Some(v) = raw.get("minConfidence").and_then(|v| v.as_f64()) {
        c.min_confidence = v;
    }
    if let Some(Value::Object(m)) = raw.get("checks") {
        c.checks = m.clone();
    }
    if let Some(Value::Array(hosts)) = raw.get("policedHosts") {
        c.policed_hosts = hosts
            .iter()
            .filter_map(|h| h.as_str().map(String::from))
            .collect();
    }
    c
}

// ── Message → tool record ────────────────────────────────────────────────────

/// Tools that don't count as a real external action for the zero-tool check.
fn nonbacking() -> &'static HashSet<&'static str> {
    static N: OnceLock<HashSet<&'static str>> = OnceLock::new();
    N.get_or_init(|| {
        [
            "memory",
            "todo",
            "think",
            "skill_manage",
            "session_search",
            "tool_search",
            "tool_describe",
            "search_knowledge",
        ]
        .into_iter()
        .collect()
    })
}

const RESULTS_CAP: usize = 200_000;

/// Transport/availability errors in a tool RESULT ground a real outage claim.
/// App errors ("document not found") deliberately don't match.
fn transport_error_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        js(
            r"econnrefused|econnreset|etimedout|enotfound|ehostunreach|connection (?:refused|reset|error|timed? ?out|aborted)|could ?n'?t connect|failed to (?:connect|fetch|reach)|network (?:error|unreachable)|socket hang up|timeout|timed out|unreachable|service unavailable|bad gateway|gateway timeout|\b50[234]\b|\b-32000\b|\bfetch failed\b|server (?:error|is down|unavailable)|temporarily unavailable",
            r"(?i)",
        )
    })
}

#[derive(Debug, Clone, Default)]
pub struct ToolRecord {
    /// Backing tools that ran this turn (excludes think/memory/todo/…).
    pub backing_tools: Vec<String>,
    /// Concatenated tool-result text this turn ("" when it overflowed).
    pub results_text: String,
    /// A tool returned a transport/availability error this turn.
    pub any_error: bool,
    /// Results too large to fully inspect → skip the grounding check (fail open).
    pub overflowed: bool,
}

/// `asText`: string content is itself; null/absent is ''; anything else is its
/// JSON serialization (with preserve_order, object key order matches JS).
fn as_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Derive the turn's tool record from the request messages: everything since
/// the last user message. Works for any OpenAI-style tool loop, any model.
/// `anyError` tests the FULL joined results; only the stored text is zeroed
/// on overflow — that is TS's order of operations, not an accident.
pub fn extract_tool_record(messages: &[Value]) -> ToolRecord {
    let mut start = 0;
    for (i, m) in messages.iter().enumerate().rev() {
        if m.get("role").and_then(|r| r.as_str()) == Some("user") {
            start = i + 1;
            break;
        }
    }
    let mut backing_tools: Vec<String> = Vec::new();
    let mut results: Vec<String> = Vec::new();
    for m in &messages[start..] {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role == "assistant"
            && let Some(calls) = m.get("tool_calls").and_then(|t| t.as_array())
        {
            for tc in calls {
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                if !name.is_empty() && !nonbacking().contains(name) {
                    backing_tools.push(name.to_string());
                }
            }
        }
        if role == "tool" || role == "function" {
            results.push(as_text(m.get("content").unwrap_or(&Value::Null)));
        }
    }
    let results_text = results.join("\n");
    let overflowed = results_text.encode_utf16().count() > RESULTS_CAP;
    let any_error = transport_error_re().is_match(&results_text);
    ToolRecord {
        backing_tools,
        results_text: if overflowed {
            String::new()
        } else {
            results_text
        },
        any_error,
        overflowed,
    }
}

// ── Grounding ────────────────────────────────────────────────────────────────

const INPUT_CAP: usize = 200_000;
const GROUNDING_CAP: usize = 1_000_000;
/// A normalized span shorter than this would ground against almost anything
/// once the separators are gone; every detector shape is longer, so the floor
/// costs nothing and rules out a degenerate match.
const GROUNDING_MIN: usize = 8;

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// First `cap` UTF-16 units of `s`, cut at a char boundary.
fn utf16_head(s: &str, cap: usize) -> &str {
    if utf16_len(s) <= cap {
        return s;
    }
    let mut units = 0usize;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > cap {
            return &s[..i];
        }
    }
    s
}

/// Last `cap` UTF-16 units of `s`, cut at a char boundary.
fn utf16_tail(s: &str, cap: usize) -> &str {
    if utf16_len(s) <= cap {
        return s;
    }
    let mut units = 0usize;
    for (i, c) in s.char_indices().rev() {
        units += c.len_utf16();
        if units > cap {
            return &s[i + c.len_utf8()..];
        }
    }
    s
}

/// Lowercase, then strip everything that isn't [a-z0-9] — the load-bearing
/// normalization: it grounds `4111-1111-1111-1111` in an answer against
/// `4111 1111 1111 1111` in the transcript it was summarizing, and catches a
/// concatenated numeric "card" the input never held in that arrangement.
fn normalize_for_grounding(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        .collect()
}

/// The normalized haystack, built once per guarded call (TS memoizes one
/// entry for the same reason: redaction asks the same question again).
pub struct Grounding {
    haystack: String,
}

impl Grounding {
    pub fn new(input: &str) -> Self {
        Grounding {
            haystack: normalize_for_grounding(utf16_head(input, GROUNDING_CAP)),
        }
    }

    /// Did this span come out of the turn's own input?
    pub fn is_grounded(&self, span: &str) -> bool {
        let needle = normalize_for_grounding(span);
        if needle.chars().count() < GROUNDING_MIN {
            return false;
        }
        self.haystack.contains(&needle)
    }
}

/// THE WHOLE INPUT of a request: everything put in front of the model this
/// turn EXCEPT what the model said — assistant turns are out on purpose, or a
/// model launders an invented card by emitting it twice (runHarness repairs by
/// appending the model's own rejected reply). The TAIL is kept when enormous:
/// a turn's own material is at the end of its prompt, and losing the head only
/// means grounding LESS, which is the pre-grounding behavior.
pub fn grounding_text_of(messages: &[Value]) -> String {
    let joined = messages
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("assistant"))
        .map(|m| as_text(m.get("content").unwrap_or(&Value::Null)))
        .collect::<Vec<_>>()
        .join("\n");
    utf16_tail(&joined, INPUT_CAP).to_string()
}

// ── Claim heuristics (ported faithfully from confab-guard) ──────────────────

// THE VOCABULARY IS THE RULE: what zero_tool_claim can SEE is exactly this
// list. An artifact must be a thing that CANNOT EXIST WITHOUT A SYSTEM ACTION
// (a ticket, a deploy, a refund); summary-shaped nouns would flag models for
// writing prose. `ran` is missing on purpose — "I ran into a problem while
// drafting the email" pairs `ran` with `email` inside the window and fires on
// a model reporting a difficulty.
const ARTIFACT: &str = concat!(
    r"draft|e-?mails?|messages?|repl(?:y|ies)|events?|meetings?|invites?|calendar|tickets?|work items?|tasks?|records?|contacts?|compan(?:y|ies)|deals?|opportunit(?:y|ies)|notes?|documents?|docs?|pages?|wiki|filters?|labels?|broadcasts?|posts?|comments?|files?|folders?|spreadsheets?|schedules?|bookings?|reminders?",
    r"|pull requests?|PRs?|branch(?:es)?|commits?|deploys?|deployments?|releases?|migrations?|rollbacks?",
    r"|refunds?|invoices?|charges?|subscriptions?|payments?",
    r"|boards?|columns?|sprints?|milestones?|dependenc(?:y|ies)|watchers?|reviews?|approvals?",
    r"|channels?|threads?|workflows?|integrations?|webhooks?",
);
const DONE_VERB: &str = concat!(
    r"created|made|drafted|set up|saved|sent|queued|posted|added|updated|edited|filed|logged|scheduled|booked|archived|moved|assigned|uploaded|published|submitted|labell?ed|starred|deleted|removed|put together|wrote up|prepared|dropped",
    r"|closed|completed|finished|resolved|fixed|merged|pushed|deployed|shipped|released|reverted|rolled back|restored",
    r"|renamed|tagged|linked|attached|shared|invited|granted|revoked|approved|rejected|triaged|reassigned|escalated|marked",
    r"|processed|issued|refunded|cancell?ed|imported|exported|synced|migrated|pinned",
);

fn claim_verb_art() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        js(
            &format!(r"\b(?:{DONE_VERB})\b[^.!?\n]{{0,40}}?\b(?:{ARTIFACT})\b"),
            r"(?i)",
        )
    })
}

fn claim_art_state() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        js(
            &format!(
                r"\b(?:{ARTIFACT})\b[^.!?\n]{{0,40}}?\b(?:is|are|has been|have been|'s)\b[^.!?\n]{{0,30}}?\b(?:created|saved|sent|done|ready|in your (?:drafts?|calendar|inbox)|on your (?:board|calendar))\b"
            ),
            r"(?i)",
        )
    })
}

fn claim_landed() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        js(
            r"\b(?:in|sitting in|added to|on)\s+your\s+(?:drafts?|calendar|board|inbox)\b",
            r"(?i)",
        )
    })
}

/// A sentence that OFFERS rather than claims — skipped by every claim
/// heuristic. `I can` AND `I could` carry a negation exception, and it was a
/// real evasion: "the gateway is rate limited, so I could not get an answer"
/// is past-tense INABILITY, not a future offer — without the exception the
/// fabricated outage in the first half went unscored. The exceptions are JS
/// lookaheads, checked by hand here; the trailing \b of TS's group is checked
/// as "the rest does not start with a word character".
fn future_plain() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        js(
            r"\b(?:I'?ll|I will|I'?d|I am going to|I'?m going to|going to|want me to|shall I|should I|would you like|do you want|ready to|happy to|I plan to|next I'?ll|let me know if)\b",
            r"(?i)",
        )
    })
}

fn can_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r"(?i)\bI can", ""))
}

fn could_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r"(?i)\bI could", ""))
}

fn can_excludes() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r"^(?:not|'?t)\b|^\s+not\b", ""))
}

fn could_excludes() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r"^n'?t\b|^\s+not\b", ""))
}

fn starts_word_char(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_future(s: &str) -> bool {
    if future_plain().is_match(s) {
        return true;
    }
    let checks = [(can_re(), can_excludes()), (could_re(), could_excludes())];
    for (word, excludes) in checks {
        for m in word.find_iter(s) {
            let rest = &s[m.end()..];
            if !starts_word_char(rest) && !excludes.is_match(rest) {
                return true;
            }
        }
    }
    false
}

/// JS SENT_SPLIT is a lookbehind (`(?<=[.!?\n])\s+`) — hand-rolled: a sentence
/// boundary is a whitespace run whose preceding char is . ! ? or \n.
fn split_sentences(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].1.is_whitespace() {
            let mut j = i;
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
            if i > 0 && matches!(chars[i - 1].1, '.' | '!' | '?' | '\n') {
                out.push(&text[start..chars[i].0]);
                start = if j < chars.len() {
                    chars[j].0
                } else {
                    text.len()
                };
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.push(&text[start..]);
    out
}

fn first_sentence(text: &str, test: impl Fn(&str) -> bool) -> Option<String> {
    for sent in split_sentences(text) {
        let s = sent.trim();
        if s.is_empty() || is_future(s) {
            continue;
        }
        if test(s) {
            return Some(s.to_string());
        }
    }
    None
}

fn claims_completed_action(text: &str) -> Option<String> {
    first_sentence(text, |s| {
        claim_verb_art().is_match(s) || claim_art_state().is_match(s) || claim_landed().is_match(s)
    })
}

// The subjects an agent in this product actually blames — the gateway, the
// provider, the search index, the queue — so "the provider is rate limited"
// (when nothing errored) does not read as ordinary prose.
const SUBJECT: &str = concat!(
    r"server|service|endpoint|API|MCP|tool|connection|backend|host|database|it|they|things",
    r"|gateway|provider|upstream|model|integration|webhook|index|queue|worker|search|sync",
);
const OUTAGE_STATE: &str = concat!(
    r"down|offline|unreachable|unavailable|not responding|won'?t respond|timing out|timed out|erroring|throwing (?:connection )?errors|stuck(?: in a recovery loop)?|in a recovery loop|flaky|went (?:down|unreachable|offline)|having (?:issues|problems|trouble)|acting up|recovering|coming back up|back up",
    r"|failing|broken|not working|refusing|rate[- ]?limited|throttled|degraded|out of service|overloaded|at capacity",
);

fn outage_patterns() -> &'static Vec<Regex> {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            js(
                &format!(
                    r"\b(?:{SUBJECT})\b[^.!?\n]{{0,40}}?\b(?:is|are|was|were|seems?|appears?|keeps?|been|being|currently|temporarily|still|right now|going|went)\b[^.!?\n]{{0,30}}?\b(?:{OUTAGE_STATE})\b"
                ),
                r"(?i)",
            ),
            js(
                &format!(r"\b(?:{SUBJECT})\b[^.!?\n]{{0,20}}?\b(?:{OUTAGE_STATE})\b"),
                r"(?i)",
            ),
            js(
                r"\b(?:can'?t|cannot|could ?n'?t|unable to|failed to|won'?t let me)\b[^.!?\n]{0,30}?\b(?:reach|connect(?: to)?|access|complete|proceed|touch|do that|delete)\b",
                r"(?i)",
            ),
            js(
                r"\bconnection (?:errors?|issues?|problems?|refused|reset|timed? ?out)\b|\b50[234]\b|\bbad gateway\b|\bgateway timeout\b",
                r"(?i)",
            ),
            js(
                r"\bauto[- ]?retry\b|\bretry (?:should be|will be|is) (?:available|possible)\b|\btry again in (?:about |~)?\d+\s*(?:second|minute|sec|min)s?\b|\bavailable (?:again )?in (?:about |~)?\d+\s*(?:second|minute|sec|min)s?\b",
                r"(?i)",
            ),
        ]
    })
}

fn claims_infra_failure(text: &str) -> Option<String> {
    first_sentence(text, |s| outage_patterns().iter().any(|p| p.is_match(s)))
}

// ── Ungrounded references ────────────────────────────────────────────────────

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r#"https?://[^\s<>"'`)\]}]+"#, r"(?i)"))
}

fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        js(
            r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
            r"(?i)",
        )
    })
}

/// Port of urlHostTail: trim, strip trailing punctuation, strip the scheme
/// (case-insensitively), cut at # then ?, split host from tail.
fn url_host_tail(url: &str) -> (String, String) {
    let mut u = url
        .trim()
        .trim_end_matches(|c: char| ".,;:!?".contains(c))
        .to_string();
    for scheme in ["https://", "http://"] {
        if u.len() >= scheme.len() && u[..scheme.len()].eq_ignore_ascii_case(scheme) {
            u = u[scheme.len()..].to_string();
            break;
        }
    }
    u = u.split('#').next().unwrap_or("").to_string();
    u = u.split('?').next().unwrap_or("").to_string();
    let slash = u.find('/');
    let host = match slash {
        Some(i) => u[..i].to_lowercase(),
        None => u.to_lowercase(),
    };
    let tail = u.to_lowercase();
    let tail = tail.trim_end_matches('/').to_string();
    (host, tail)
}

/// Policed URLs (internal-host with a path) + UUIDs found in text, lowercased.
fn extract_refs(text: &str, policed_hosts: &[String]) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    for m in url_re().find_iter(text) {
        let (host, tail) = url_host_tail(m.as_str());
        let policed = policed_hosts
            .iter()
            .any(|h| host == h.to_lowercase() || host.ends_with(&h.to_lowercase()));
        if policed && tail.contains('/') {
            refs.push(tail);
        }
    }
    for m in uuid_re().find_iter(text) {
        refs.push(m.as_str().to_lowercase());
    }
    let mut seen: Vec<String> = Vec::new();
    for r in refs {
        if !seen.contains(&r) {
            seen.push(r);
        }
    }
    seen
}

fn ungrounded_refs(text: &str, haystack: &str, policed_hosts: &[String]) -> Vec<String> {
    let hay = haystack
        .to_lowercase()
        .replace("https://", "")
        .replace("http://", "");
    extract_refs(text, policed_hosts)
        .into_iter()
        .filter(|r| !hay.contains(r.as_str()))
        .collect()
}

// ── Secret / PII detection ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ShapeHit {
    label: String,
    snippet: String,
    grounded: bool,
}

/// The first UNGROUNDED credential in the text, or — when every one of them
/// came out of the input — the first grounded one, marked. Never just the
/// first hit: a reply that quotes the key the user pasted AND emits one of its
/// own must report the one the model invented.
fn detect_secret(text: &str, grounding: &Grounding) -> Option<ShapeHit> {
    let mut grounded: Option<ShapeHit> = None;
    for p in vault::compiled() {
        for m in p.re.find_iter(text) {
            let raw = m.as_str();
            let hit = ShapeHit {
                label: p.label.to_string(),
                snippet: format!("{}: {}…", p.label, raw.chars().take(8).collect::<String>()),
                grounded: grounding.is_grounded(raw),
            };
            if !hit.grounded {
                return Some(hit);
            }
            grounded = grounded.or(Some(hit));
        }
    }
    grounded
}

fn luhn(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut dbl = false;
    for c in digits.chars().rev() {
        let mut d = c.to_digit(10).unwrap_or(0);
        if dbl {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        dbl = !dbl;
    }
    sum.is_multiple_of(10)
}

fn ssn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r"\b\d{3}-\d{2}-\d{4}\b", ""))
}

/// TS's SSN pattern carries three negative lookaheads (area ∉ {000, 666, 9xx},
/// group ≠ 00, serial ≠ 0000) — hand-checked here.
fn ssn_valid(m: &str) -> bool {
    let parts: Vec<&str> = m.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    !(parts[0] == "000" || parts[0] == "666" || parts[0].starts_with('9'))
        && parts[1] != "00"
        && parts[2] != "0000"
}

fn card_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| js(r"\b(?:\d[ -]?){12,18}\d\b", ""))
}

fn iban_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // No (?i) in TS: IBAN country codes and the body are uppercase-only.
        js(
            r"\b(?:DE|FR|GB|NL|ES|IT|CH|AT|BE|PT|IE|PL|SE|NO|DK|FI)\d{2}[A-Z0-9]{10,30}\b",
            "",
        )
    })
}

fn is_card_number(raw: &str) -> bool {
    let digits: String = raw.chars().filter(|c| *c != ' ' && *c != '-').collect();
    (13..=19).contains(&digits.len()) && luhn(&digits)
}

fn digits_of(raw: &str) -> String {
    raw.chars().filter(|c| *c != ' ' && *c != '-').collect()
}

/// Same rule as detect_secret: the first UNGROUNDED hit wins, in the SSN →
/// card → IBAN priority the labels have always had; a wholly grounded text
/// reports its first hit marked rather than nothing.
fn detect_pii(text: &str, grounding: &Grounding) -> Option<ShapeHit> {
    let mut grounded: Option<ShapeHit> = None;
    let mut take = |hit: ShapeHit| -> Option<ShapeHit> {
        if !hit.grounded {
            return Some(hit);
        }
        if grounded.is_none() {
            grounded = Some(hit);
        }
        None
    };
    for m in ssn_re().find_iter(text) {
        if !ssn_valid(m.as_str()) {
            continue;
        }
        if let Some(hit) = take(ShapeHit {
            label: "social security number".into(),
            snippet: format!("SSN: {}…", m.as_str().chars().take(6).collect::<String>()),
            grounded: grounding.is_grounded(m.as_str()),
        }) {
            return Some(hit);
        }
    }
    for m in card_re().find_iter(text) {
        if !is_card_number(m.as_str()) {
            continue;
        }
        if let Some(hit) = take(ShapeHit {
            label: "payment card number".into(),
            snippet: format!(
                "card: {}…",
                digits_of(m.as_str()).chars().take(6).collect::<String>()
            ),
            grounded: grounding.is_grounded(m.as_str()),
        }) {
            return Some(hit);
        }
    }
    for m in iban_re().find_iter(text) {
        if let Some(hit) = take(ShapeHit {
            label: "bank account (IBAN)".into(),
            snippet: format!("IBAN: {}…", m.as_str().chars().take(6).collect::<String>()),
            grounded: grounding.is_grounded(m.as_str()),
        }) {
            return Some(hit);
        }
    }
    grounded
}

// ── Findings, rules, evaluation ──────────────────────────────────────────────

/// WHAT A GROUNDED HIT CHANGES, declared by the rule that produced it:
///   Finding            drop the FINDING, keep the REDACTION (secret_leak —
///                      a real key that was really pasted is not evidence
///                      about the model, but Talaria still refuses to write a
///                      second copy of it).
///   FindingAndRedaction drop both (pii_leak — the span is most likely an
///                      order number the model read out of the user's own
///                      document; rewriting it makes the artifact less true).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Groundable {
    Finding,
    FindingAndRedaction,
}

/// Serialized for the callers that PIN findings to their own rows
/// (`channel_messages.guard`): key order is the TS interface's, and `grounded`
/// is absent-when-false exactly as the TS optional is.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub check: &'static str,
    pub severity: &'static str,
    pub confidence: f64,
    pub message: String,
    pub snippet: String,
    /// The flagged span was already in the turn's input — not evidence about
    /// the model. Kept only so `needs_redaction` still says yes; never
    /// recorded, never disclosed.
    #[serde(skip_serializing_if = "is_false")]
    pub grounded: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The audience a reply is about to reach (guardrails.ts `GuardContext.spread`).
/// CONTAINED is the 1:1 default — a DM back to the person who pasted the data.
/// BROADCAST is a channel: the reply lands in front of everyone in the room and
/// in the retrieval index behind it, an audience the source material never
/// had, so the "it is already in the ticket anyway" grounding exemption does
/// not hold — a grounded finding+redaction hit SURVIVES (as grounded: it still
/// is not evidence about the model), and redaction under broadcast passes no
/// grounding at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spread {
    Contained,
    Broadcast,
}

/// What a caller can honestly say about the turn's tool results. A path with
/// the full record has both; the harness guard pass holds a tool NAMES list
/// and derives its record from it — `results` it can reconstruct, `errorInfo`
/// (guardrails.ts: "did anything error?") it cannot, because a name is not an
/// outcome. A rule whose `needs` cannot be supplied is SKIPPED (no false
/// positive) rather than run on missing data — the same posture as
/// `gate_safe`, one level up.
pub struct Available {
    pub results: bool,
    pub error_info: bool,
}

/// The caller that really has the whole record — the gateway completion path.
pub const FULL: Available = Available {
    results: true,
    error_info: true,
};

/// One unit of `Available` a rule declares it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Results,
    ErrorInfo,
}

struct RuleDef {
    id: &'static str,
    severity: &'static str,
    default_on: bool,
    groundable: Option<Groundable>,
    /// Runs over plain text with no tool record (guardrails.ts gateSafe) —
    /// the rules `guard_text` may use. The tool-record rules (zero_tool_claim,
    /// ungrounded_ref, fabricated_outage) have nothing to be true AGAINST on
    /// that path: an MCP comment claiming "I opened the PR" is backed by a
    /// tool that ran in a different process, so running them would flag
    /// honest work. Rules that need what we cannot supply are skipped rather
    /// than guessed.
    gate_safe: bool,
    /// Which halves of `Available` this rule's truth depends on. Empty for
    /// every rule but the two tool-record ones; `zero_tool_claim` needs
    /// neither because an empty record is its answer, not missing data.
    needs: &'static [Need],
}

const RULES: &[RuleDef] = &[
    RuleDef {
        id: "zero_tool_claim",
        severity: "high",
        default_on: true,
        groundable: None,
        gate_safe: false,
        needs: &[],
    },
    RuleDef {
        id: "ungrounded_ref",
        severity: "medium",
        default_on: true,
        groundable: None,
        gate_safe: false,
        needs: &[Need::Results],
    },
    RuleDef {
        id: "fabricated_outage",
        severity: "high",
        default_on: true,
        groundable: None,
        gate_safe: false,
        needs: &[Need::ErrorInfo],
    },
    RuleDef {
        id: "secret_leak",
        severity: "high",
        default_on: true,
        groundable: Some(Groundable::Finding),
        gate_safe: true,
        needs: &[],
    },
    RuleDef {
        id: "pii_leak",
        severity: "high",
        default_on: true,
        groundable: Some(Groundable::FindingAndRedaction),
        gate_safe: true,
        needs: &[],
    },
];

/// The context one guarded completion runs against. `input_text` is the RAW
/// grounding text (ungrounded_ref matches refs against it verbatim, lowercased
/// only); `grounding` is its normalized form for span grounding. The gateway
/// always supplies the full input (`grounding_text_of`), so GuardContext's
/// narrow `userMessage` fallback has no caller here. `spread` is Contained on
/// every gateway path; the comms reply loop is the broadcast caller.
pub struct GuardContext<'a> {
    pub answer: &'a str,
    pub tool_record: &'a ToolRecord,
    pub input_text: &'a str,
    pub policed_hosts: &'a [String],
    pub grounding: &'a Grounding,
    pub spread: Spread,
}

/// A rule's raw hit, before the confidence threshold and grounding decision.
struct Hit {
    message: String,
    snippet: String,
    confidence: f64,
    grounded: bool,
}

fn run_rule(id: &str, ctx: &GuardContext) -> Option<Hit> {
    match id {
        "zero_tool_claim" => {
            if !ctx.tool_record.backing_tools.is_empty() {
                return None;
            }
            claims_completed_action(ctx.answer).map(|s| Hit {
                message: "Claims a completed action, but no external tool ran this turn.".into(),
                snippet: s.chars().take(240).collect(),
                confidence: 0.8,
                grounded: false,
            })
        }
        "ungrounded_ref" => {
            let tr = ctx.tool_record;
            if tr.backing_tools.is_empty() || tr.overflowed {
                return None;
            }
            let haystack = format!("{}\n{}", tr.results_text, ctx.input_text);
            let ungrounded = ungrounded_refs(ctx.answer, &haystack, ctx.policed_hosts);
            (!ungrounded.is_empty()).then(|| Hit {
                message: "Cites link(s)/id(s) that did not appear in any tool result this turn. They may be fabricated.".into(),
                snippet: ungrounded.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                confidence: 0.7,
                grounded: false,
            })
        }
        "fabricated_outage" => {
            if ctx.tool_record.any_error {
                return None;
            }
            claims_infra_failure(ctx.answer).map(|s| Hit {
                message: "Claims an outage/failure, but no tool returned an error this turn."
                    .into(),
                snippet: s.chars().take(240).collect(),
                confidence: 0.85,
                grounded: false,
            })
        }
        "secret_leak" => detect_secret(ctx.answer, ctx.grounding).map(|h| Hit {
            message: format!("Output appears to contain a live credential ({}).", h.label),
            snippet: h.snippet,
            confidence: 0.95,
            grounded: h.grounded,
        }),
        "pii_leak" => detect_pii(ctx.answer, ctx.grounding).map(|h| Hit {
            message: format!("Output appears to contain personal data ({}).", h.label),
            snippet: h.snippet,
            confidence: 0.9,
            grounded: h.grounded,
        }),
        _ => None,
    }
}

/// One rule against one context: the confidence threshold, then the grounding
/// decision — the ONLY place Groundable is read, so callers cannot drift on
/// what a grounded span costs.
fn evaluate(rule: &RuleDef, ctx: &GuardContext, config: &GuardConfig) -> Option<Finding> {
    let hit = run_rule(rule.id, ctx)?;
    if hit.confidence < config.min_confidence {
        return None;
    }
    let finding = Finding {
        check: rule.id,
        severity: rule.severity,
        confidence: hit.confidence,
        message: hit.message,
        snippet: hit.snippet,
        grounded: false,
    };
    if !hit.grounded {
        return Some(finding);
    }
    // A grounded hit from a rule that never declared itself groundable is an
    // author mistake, and the safe reading is the old behavior: an ordinary
    // finding. Grounding may only ever REMOVE a claim about the model.
    match rule.groundable? {
        Groundable::Finding => Some(Finding {
            grounded: true,
            ..finding
        }),
        // Contained: both halves drop — the data came from this person and is
        // going back to this person. Broadcast REVERSES it (guardrails.ts
        // `GuardContext.spread`): the audience argument no longer holds, so
        // the finding survives as grounded (still not evidence about the
        // model) and the persisted copy still gets scrubbed.
        Groundable::FindingAndRedaction => match ctx.spread {
            Spread::Contained => None,
            Spread::Broadcast => Some(Finding {
                grounded: true,
                ..finding
            }),
        },
    }
}

/// Run the enabled, APPLICABLE rules, keep findings at/above the threshold.
/// `available` says what the caller can honestly supply about tool results:
/// a rule whose `needs` it cannot meet is skipped — no false positive from
/// missing data. The gateway completion path passes `FULL`.
pub fn run_guardrails(
    ctx: &GuardContext,
    config: &GuardConfig,
    available: &Available,
) -> Vec<Finding> {
    if ctx.answer.is_empty() {
        return Vec::new();
    }
    RULES
        .iter()
        .filter(|r| rule_enabled(config, r))
        .filter(|r| {
            r.needs.iter().all(|n| match n {
                Need::Results => available.results,
                Need::ErrorInfo => available.error_info,
            })
        })
        .filter_map(|r| evaluate(r, ctx, config))
        .collect()
}

/// The one spelling of "is this rule on" — `checks[id] ?? default_on`, the
/// TS `ruleEnabled`. Both `run_guardrails`'s filter and `guard_text`'s read
/// it so the two loops cannot drift.
fn rule_enabled(config: &GuardConfig, rule: &RuleDef) -> bool {
    config
        .checks
        .get(rule.id)
        .and_then(|v| v.as_bool())
        .unwrap_or(rule.default_on)
}

/// A def's `guard.rules` narrowed onto the live config (run.ts
/// narrowGuardConfig): a rule runs only when the def names it AND the admin
/// left it on. `None` (no guard block) returns the config untouched. Every
/// RULES id gets an explicit entry so the narrowed config's `checks` is a
/// total map — absent would fall back to the default, which is not what a
/// def that omitted a rule means.
/// Every rule id in the registry. Its consumer is the harness registry's
/// declaration test: a def naming a rule id that does not exist does not
/// disable one rule — `narrow_guard_config` below turns a rule on only when
/// the def NAMES it, so a typo disables ALL of them, with no error anywhere
/// and a `guard` block in the file that still reads as protection. That test
/// needs the live ids, not a copy that could drift from `RULES`.
pub fn rule_ids() -> Vec<&'static str> {
    RULES.iter().map(|r| r.id).collect()
}

pub fn narrow_guard_config(config: &GuardConfig, rules: Option<&[&str]>) -> GuardConfig {
    let Some(rules) = rules else {
        return config.clone();
    };
    let mut checks = serde_json::Map::new();
    for rule in RULES {
        let on = rules.contains(&rule.id) && rule_enabled(config, rule);
        checks.insert(rule.id.to_string(), Value::Bool(on));
    }
    GuardConfig {
        checks,
        ..config.clone()
    }
}

/// Which findings warrant content redaction — a GROUNDED finding still counts:
/// it survived for exactly this predicate, so a credential the user pasted is
/// still scrubbed from what Talaria writes down.
pub fn needs_redaction(findings: &[Finding]) -> bool {
    findings
        .iter()
        .any(|f| f.check == "secret_leak" || f.check == "pii_leak")
}

/// A human-facing caveat for annotate mode, appended out-of-band — never
/// re-fed into the model's context. GROUNDED FINDINGS ARE NOT DISCLOSED: the
/// wording is a claim about the model, and a grounded span is not one.
pub fn caveat_for(findings: &[Finding]) -> String {
    let shown: Vec<&Finding> = findings.iter().filter(|f| !f.grounded).collect();
    if shown.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = shown
        .iter()
        .map(|f| {
            let display = f.check.replace('_', " ");
            let snip = if f.snippet.is_empty() {
                String::new()
            } else {
                format!(" ({})", f.snippet)
            };
            format!("- **{display}:** {}{snip}", f.message)
        })
        .collect();
    format!(
        "\n\n---\n⚠️ **Unverified: confab guard flagged this response:**\n{}\nVerify before relying on it.",
        lines.join("\n")
    )
}

// ── Findings store ───────────────────────────────────────────────────────────

/// THE ONE DOOR to guard_findings, and therefore the one place that decides
/// what counts as a fact about a model: grounded findings do not go in (the
/// column is read as a per-model confabulation rate by the fitness page).
pub async fn record_findings(
    pg: &PgPool,
    findings: &[Finding],
    caller: &str,
    model: &str,
    endpoint: Option<&str>,
    mode: GuardMode,
) {
    let mode_str = match mode {
        GuardMode::Off => "off",
        GuardMode::Observe => "observe",
        GuardMode::Annotate => "annotate",
        GuardMode::Strict => "strict",
    };
    for f in findings.iter().filter(|f| !f.grounded) {
        let _ = sqlx::query(
            "insert into guard_findings (caller, model, endpoint, mode, check_type, severity, confidence, message, snippet) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(caller)
        .bind(model)
        .bind(endpoint)
        .bind(mode_str)
        .bind(f.check)
        .bind(f.severity)
        .bind(f.confidence as f32)
        .bind(&f.message)
        .bind(&f.snippet)
        .execute(pg)
        .await;
    }
}

/// The one-call entry point for the gateway: run guards (if enabled) on a
/// finished completion, record findings, and return any annotate-mode caveat.
pub async fn guard_completion(
    pg: &PgPool,
    answer: &str,
    messages: &Value,
    caller: &str,
    model: &str,
    endpoint: Option<&str>,
) -> (Vec<Finding>, String, GuardMode) {
    let config = guard_config(pg).await;
    if config.mode == GuardMode::Off || answer.is_empty() {
        return (Vec::new(), String::new(), config.mode);
    }
    let msgs = messages.as_array().cloned().unwrap_or_default();
    let tool_record = extract_tool_record(&msgs);
    let input_text = grounding_text_of(&msgs);
    let grounding = Grounding::new(&input_text);
    let ctx = GuardContext {
        answer,
        tool_record: &tool_record,
        spread: Spread::Contained,
        input_text: &input_text,
        policed_hosts: &config.policed_hosts,
        grounding: &grounding,
    };
    let findings = run_guardrails(&ctx, &config, &FULL);
    record_findings(pg, &findings, caller, model, endpoint, config.mode).await;
    let caveat = if config.mode.discloses() {
        caveat_for(&findings)
    } else {
        String::new()
    };
    (findings, caveat, config.mode)
}

/// Layered tiering over PLAIN TEXT (guardrails.ts guardText): the gate-safe
/// rules with an empty tool record — the judge's cheap structural pre-pass and
/// the agent-writes door both call this. Returns [] when the guard is off.
///
/// `input` is the material the text was written FROM, when the caller has it
/// — the ticket being triaged, the thread being replied to. Both gate-safe
/// rules are groundable, so supplying it is the difference between flagging a
/// model for repeating the customer's order number and not; callers with
/// nothing honest to name pass None and get the old behavior.
///
/// Never fails: a guard that errored loudly on a database hiccup would take
/// down commenting, posting and DMs alongside it.
/// The comms reply guard (guardrails.ts `guardChatReply`): a streamed reply
/// the caller holds a TOOL NAMES list for — not the full record, so the record
/// is the derived one (`overflowed` — results were too big to inspect, fail
/// open on grounding) and `available` honestly says neither half is supplied.
/// `spread` is the audience argument; a CHANNEL IS A BROADCAST.
///
/// Returns the findings and the mode; `record_findings` inside drops grounded
/// ones, exactly as the TS `.catch(() => {})` call does.
pub async fn guard_chat_reply(
    pg: &PgPool,
    answer: &str,
    tool_names: &[String],
    user_message: &str,
    caller: &str,
    model: &str,
    spread: Spread,
) -> (Vec<Finding>, GuardMode) {
    let config = guard_config(pg).await;
    if config.mode == GuardMode::Off || answer.is_empty() {
        return (Vec::new(), config.mode);
    }
    let backing_tools: Vec<String> = tool_names
        .iter()
        .filter(|n| !n.is_empty() && !nonbacking().contains(n.as_str()))
        .cloned()
        .collect();
    let tool_record = ToolRecord {
        backing_tools,
        results_text: String::new(),
        any_error: false,
        overflowed: true,
    };
    let grounding = Grounding::new(user_message);
    let ctx = GuardContext {
        answer,
        tool_record: &tool_record,
        input_text: user_message,
        policed_hosts: &config.policed_hosts,
        grounding: &grounding,
        spread,
    };
    let findings = run_guardrails(
        &ctx,
        &config,
        &Available {
            results: false,
            error_info: false,
        },
    );
    record_findings(pg, &findings, caller, model, Some("fleet"), config.mode).await;
    (findings, config.mode)
}

/// redactFindings: scrub each finding's SNIPPET in place — a pinned finding
/// carries a verbatim excerpt of the flagged span, and `zero_tool_claim` does
/// not truncate its own. Contained redaction with no grounding: the snippet is
/// evidence, and broadcast already stripped the exemption upstream.
pub fn redact_findings(findings: &[Finding]) -> Vec<Finding> {
    findings
        .iter()
        .map(|f| {
            if f.snippet.is_empty() {
                f.clone()
            } else {
                Finding {
                    snippet: redact_secrets(&f.snippet, None).0,
                    ..f.clone()
                }
            }
        })
        .collect()
}

pub async fn guard_text(pg: &PgPool, text: &str, input: Option<&str>) -> Vec<Finding> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let config = guard_config(pg).await;
    gate_safe(&config, text, input)
}

/// The settings-free half of `guard_text`: the gate-safe rules over plain text,
/// against optional grounding material. Split out because the harness runner's
/// REPAIR GATE is this exact pass behind an injected edge, and its tests need
/// the real rule registry without a database — a fake gate would turn every
/// "does the runner refuse to repair a flagged reply" assertion into a
/// restatement of the fake.
pub fn gate_safe(config: &GuardConfig, text: &str, input: Option<&str>) -> Vec<Finding> {
    if text.trim().is_empty() || config.mode == GuardMode::Off {
        return Vec::new();
    }
    let empty = ToolRecord::default();
    let input_text = input.unwrap_or("");
    let grounding = Grounding::new(input_text);
    let ctx = GuardContext {
        answer: text,
        tool_record: &empty,
        input_text,
        policed_hosts: &config.policed_hosts,
        grounding: &grounding,
        spread: Spread::Contained,
    };
    RULES
        .iter()
        .filter(|r| r.gate_safe)
        .filter(|r| rule_enabled(config, r))
        .filter_map(|r| evaluate(r, &ctx, config))
        .collect()
}

// ── Redaction ────────────────────────────────────────────────────────────────

/// Replace every detected credential AND ungrounded high-precision PII with a
/// redaction marker. CREDENTIALS ARE REDACTED WHETHER OR NOT THEY ARE
/// GROUNDED (the deliberate half of secret_leak's split); PII skips a grounded
/// span — the order number in the distillation is the one from the chat, and a
/// summary in which it has become `[redacted card number]` is a worse artifact
/// than the one it replaced.
pub fn redact_secrets(text: &str, grounding: Option<&Grounding>) -> (String, bool) {
    let mut out = text.to_string();
    for p in vault::compiled() {
        let base = p.redact.as_ref().unwrap_or(&p.re);
        out = base
            .replace_all(&out, format!("[redacted {}]", p.label))
            .into_owned();
    }
    let is_grounded = |m: &str| grounding.is_some_and(|g| g.is_grounded(m));
    out = ssn_re()
        .replace_all(&out, |c: &regex::Captures| {
            let m = c.get(0).map(|g| g.as_str()).unwrap_or_default();
            if ssn_valid(m) && !is_grounded(m) {
                "[redacted SSN]".to_string()
            } else {
                m.to_string()
            }
        })
        .into_owned();
    out = card_re()
        .replace_all(&out, |c: &regex::Captures| {
            let m = c.get(0).map(|g| g.as_str()).unwrap_or_default();
            if is_card_number(m) && !is_grounded(m) {
                "[redacted card number]".to_string()
            } else {
                m.to_string()
            }
        })
        .into_owned();
    out = iban_re()
        .replace_all(&out, |c: &regex::Captures| {
            let m = c.get(0).map(|g| g.as_str()).unwrap_or_default();
            if !is_grounded(m) {
                "[redacted IBAN]".to_string()
            } else {
                m.to_string()
            }
        })
        .into_owned();
    let redacted = out != text;
    (out, redacted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msgs(list: Value) -> Vec<Value> {
        list.as_array().cloned().unwrap()
    }

    fn record(list: Value) -> ToolRecord {
        extract_tool_record(&msgs(list))
    }

    /// A no-tool context over `input` — the gateway's shape for a plain chat
    /// turn. The record and grounding must outlive the context, so callers
    /// build all three.
    fn ctx<'a>(
        answer: &'a str,
        input: &'a str,
        tr: &'a ToolRecord,
        g: &'a Grounding,
    ) -> GuardContext<'a> {
        GuardContext {
            answer,
            tool_record: tr,
            input_text: input,
            policed_hosts: &[],
            grounding: g,
            spread: Spread::Contained,
        }
    }

    #[test]
    fn tool_record_excludes_nonbacking_and_tests_errors_on_the_full_text() {
        let r = record(json!([
            {"role": "user", "content": "pull the tickets"},
            {"role": "assistant", "content": null, "tool_calls": [
                {"function": {"name": "memory"}},
                {"function": {"name": "linear_create_ticket"}},
                {"function": {"name": ""}}
            ]},
            {"role": "tool", "content": "fetch failed"},
        ]));
        assert_eq!(r.backing_tools, vec!["linear_create_ticket".to_string()]);
        assert!(r.any_error); // transport error in the result text
        assert!(!r.overflowed);
        // Everything before the last user message is not this turn.
        let r = record(json!([
            {"role": "tool", "content": "econnrefused"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "no tools ran"}
        ]));
        assert!(r.backing_tools.is_empty());
        assert!(!r.any_error);
    }

    #[test]
    fn overflow_zeroes_results_but_any_error_saw_the_full_text() {
        let big = "timeout after all — econnrefused ".repeat(10_000);
        let r = record(json!([
            {"role": "user", "content": "go"},
            {"role": "tool", "content": big},
        ]));
        assert!(r.overflowed);
        assert_eq!(r.results_text, "");
        assert!(r.any_error); // tested on the FULL joined text, before zeroing
    }

    #[test]
    fn grounding_normalizes_separators_and_keeps_the_head_of_the_haystack() {
        let g = Grounding::new("customer said: 4111 1111 1111 1111 on the phone");
        assert!(g.is_grounded("4111-1111-1111-1111"));
        assert!(!g.is_grounded("1234")); // below GROUNDING_MIN
        // GROUNDING_CAP keeps the HEAD: a marker past the cap is cut away.
        let huge = format!("{}{}", "a".repeat(GROUNDING_CAP + 10), "marker1234");
        let g = Grounding::new(&huge);
        assert!(!g.is_grounded("marker1234"));
        assert!(g.is_grounded(&"a".repeat(GROUNDING_MIN)));
    }

    #[test]
    fn grounding_text_drops_assistant_turns_and_keeps_the_tail() {
        let text = grounding_text_of(&msgs(json!([
            {"role": "system", "content": "sys"},
            {"role": "assistant", "content": "model said 4111-1111-1111-1111"},
            {"role": "user", "content": "user said 4111-1111-1111-1111"},
        ])));
        assert!(text.contains("sys"));
        assert!(!text.contains("model said"));
        assert!(text.contains("user said"));
        // An enormous history keeps only the tail.
        let long = "b".repeat(INPUT_CAP + 10);
        let text = grounding_text_of(&msgs(json!([
            {"role": "system", "content": format!("HEADMARK {long} TAILMARK")},
        ])));
        assert!(text.contains("TAILMARK"));
        assert!(!text.contains("HEADMARK"));
    }

    #[test]
    fn zero_tool_claim_fires_on_done_verb_plus_artifact() {
        let s = claims_completed_action("I closed PLAT-118 and merged the migration PR. On it!")
            .unwrap();
        assert!(s.starts_with("I closed PLAT-118"));
    }

    #[test]
    fn an_offer_is_not_a_claim_and_inability_is_not_an_offer() {
        // "I can close…" is an offer — skipped entirely.
        assert!(claims_completed_action("I can close the ticket once you confirm.").is_none());
        // The load-bearing negation exception: "…so I could not get an answer"
        // is past inability — the sentence is NOT skipped, and its first half
        // is an outage claim with nothing having errored.
        let outage =
            claims_infra_failure("The gateway is rate limited, so I could not get an answer.")
                .unwrap();
        assert!(outage.starts_with("The gateway is rate limited"));
        assert!(is_future("I can."));
        assert!(!is_future("I can't close the ticket"));
        assert!(!is_future("I cannot close the ticket"));
        assert!(!is_future("I can not close the ticket"));
        assert!(!is_future("I couldn't finish"));
        assert!(is_future("I could reschedule if you want"));
    }

    #[test]
    fn fabricated_outage_needs_no_real_error_and_finds_the_blameable_subjects() {
        assert!(claims_infra_failure("The provider is rate limited right now.").is_some());
        assert!(claims_infra_failure("The search index is degraded, try later.").is_some());
        assert!(claims_infra_failure("I love this service.").is_none());
    }

    #[test]
    fn ungrounded_ref_checks_policed_hosts_and_uuids() {
        let hosts = vec!["linear.app".to_string()];
        let hay = "see https://linear.app/ticket/123 and 550e8400-e29b-41d4-a716-446655440000";
        let found = ungrounded_refs(
            "fixed https://linear.app/ticket/999 and 550e8400-e29b-41d4-a716-446655440000",
            hay,
            &hosts,
        );
        assert_eq!(found, vec!["linear.app/ticket/999".to_string()]);
        assert!(ungrounded_refs("see https://example.com/x/1", "", &hosts).is_empty());
    }

    #[test]
    fn secret_detection_reports_the_invented_key_not_the_quoted_one() {
        let key = "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let input = format!("rotate this: {key}");
        let g = Grounding::new(&input);
        let answer = format!(
            "your key {key} is rotated, and here is a new one: sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        let hit = detect_secret(&answer, &g).unwrap();
        assert!(!hit.grounded); // the invented one, not the quoted one
        assert_eq!(hit.label, "Anthropic key");
        assert_eq!(hit.snippet, "Anthropic key: sk-ant-a…");
        // A wholly grounded text reports its first hit marked instead of null.
        let hit = detect_secret(&format!("your key {key} again"), &g).unwrap();
        assert!(hit.grounded);
        assert!(hit.snippet.starts_with("GitHub token: ghp_aaaa…"));
    }

    #[test]
    fn pii_shapes_validate_like_the_js_lookaheads() {
        assert!(ssn_valid("536-90-4449"));
        assert!(!ssn_valid("000-90-4449")); // area 000
        assert!(!ssn_valid("666-90-4449")); // area 666
        assert!(!ssn_valid("900-90-4449")); // area 9xx
        assert!(!ssn_valid("536-00-4449")); // group 00
        assert!(!ssn_valid("536-90-0000")); // serial 0000
        assert!(is_card_number("4111-1111-1111-1111")); // Luhn-valid
        assert!(!is_card_number("4111-1111-1111-1112")); // one off
        assert!(iban_re().is_match("DE89370400440532013000"));
        assert!(!iban_re().is_match("de89370400440532013000")); // lowercase is not an IBAN in TS
    }

    #[test]
    fn caveat_and_redaction_shapes() {
        let f = Finding {
            check: "zero_tool_claim",
            severity: "high",
            confidence: 0.8,
            message: "Claims a completed action.".into(),
            snippet: "I closed it.".into(),
            grounded: false,
        };
        assert_eq!(
            caveat_for(std::slice::from_ref(&f)),
            "\n\n---\n⚠️ **Unverified: confab guard flagged this response:**\n- **zero tool claim:** Claims a completed action. (I closed it.)\nVerify before relying on it."
        );
        // A grounded-only finding discloses nothing.
        let g = Finding {
            grounded: true,
            ..f.clone()
        };
        assert_eq!(caveat_for(std::slice::from_ref(&g)), "");
        // But it still warrants redaction.
        let secret = Finding {
            check: "secret_leak",
            ..g
        };
        assert!(needs_redaction(&[secret]));
        // Redaction: credentials go unconditionally, cards only ungrounded.
        let input = Grounding::new("order 4111 1111 1111 1111 shipped");
        let (out, redacted) = redact_secrets(
            "card 4111-1111-1111-1111 and key ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa and DE89370400440532013000",
            Some(&input),
        );
        assert!(redacted);
        assert!(out.contains("card 4111-1111-1111-1111")); // grounded — kept
        assert!(out.contains("[redacted GitHub token]"));
        assert!(out.contains("[redacted IBAN]"));
    }

    #[test]
    fn evaluate_drops_grounded_pii_and_keeps_grounded_secrets_marked() {
        let config = GuardConfig {
            mode: GuardMode::Strict,
            checks: serde_json::Map::new(),
            min_confidence: 0.5,
            policed_hosts: vec![],
        };
        let input = "order 4111 1111 1111 1111 shipped, key ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa pasted";
        let tr = ToolRecord::default();
        let g = Grounding::new(input);
        // A grounded card is not PII evidence: pii_leak drops it entirely.
        let c = ctx("card 4111-1111-1111-1111", input, &tr, &g);
        assert!(run_guardrails(&c, &config, &FULL).is_empty());
        // A grounded secret still returns, marked — needs_redaction says yes.
        let c = ctx(
            "key ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            input,
            &tr,
            &g,
        );
        let findings = run_guardrails(&c, &config, &FULL);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].check, "secret_leak");
        assert!(findings[0].grounded);
        assert!(needs_redaction(&findings));
    }

    #[test]
    fn gate_safe_is_exactly_the_two_leak_rules() {
        // guard_text's door: the rules that need no tool record. The three
        // claim rules are pinned OUT of it, or an MCP comment saying "I opened
        // the PR" would flag honest work.
        let gate_safe: Vec<&str> = RULES.iter().filter(|r| r.gate_safe).map(|r| r.id).collect();
        assert_eq!(gate_safe, vec!["secret_leak", "pii_leak"]);
    }

    #[test]
    fn rules_respect_checks_and_confidence() {
        let mut config = GuardConfig {
            mode: GuardMode::Observe,
            checks: serde_json::Map::new(),
            min_confidence: 0.99, // above every rule's confidence
            policed_hosts: vec![],
        };
        let tr = ToolRecord::default();
        let g = Grounding::new("");
        let c = ctx("I closed the ticket.", "", &tr, &g);
        assert!(run_guardrails(&c, &config, &FULL).is_empty());
        config.min_confidence = 0.5;
        config.checks.insert("zero_tool_claim".into(), json!(false));
        assert!(run_guardrails(&c, &config, &FULL).is_empty());
        config.checks.insert("zero_tool_claim".into(), json!(true));
        assert_eq!(run_guardrails(&c, &config, &FULL).len(), 1);
    }

    #[test]
    fn unsupplied_needs_skip_the_rule_not_the_run() {
        // The harness guard pass's honest posture: tool NAMES are results,
        // a name is not an outcome. fabricated_outage (needs errorInfo) must
        // stand down rather than flag a reply it cannot check.
        let config = GuardConfig {
            mode: GuardMode::Observe,
            checks: serde_json::Map::new(),
            min_confidence: 0.5,
            policed_hosts: vec![],
        };
        let tr = ToolRecord {
            backing_tools: vec!["t".into()],
            results_text: "r".into(),
            any_error: false,
            overflowed: false,
        };
        let g = Grounding::new("");
        // A reply that would trip BOTH tool-record rules: an outage claim with
        // nothing errored, and an id no result contains (URLs police only on
        // configured hosts; a UUID is a ref unconditionally). FULL flags both.
        let c = ctx(
            "The API is down. Ticket 123e4567-e89b-12d3-a456-426614174000 was filed.",
            "",
            &tr,
            &g,
        );
        let ids = |a: &Available| -> Vec<&'static str> {
            run_guardrails(&c, &config, a)
                .iter()
                .map(|f| f.check)
                .collect()
        };
        assert_eq!(ids(&FULL), vec!["ungrounded_ref", "fabricated_outage"]);
        // The harness guard pass's honest posture: tool NAMES are results, a
        // name is not an outcome. fabricated_outage stands down; the link
        // check still runs on the record the names reconstruct.
        let names_only = Available {
            results: true,
            error_info: false,
        };
        assert_eq!(ids(&names_only), vec!["ungrounded_ref"]);
        // Starving results stands down the link check instead — each half
        // gates only its own rule, never the whole run.
        let no_results = Available {
            results: false,
            error_info: true,
        };
        assert_eq!(ids(&no_results), vec!["fabricated_outage"]);
    }

    #[test]
    fn narrow_is_def_rules_and_admin_checks() {
        let mut config = GuardConfig {
            mode: GuardMode::Observe,
            checks: serde_json::Map::new(),
            min_confidence: 0.5,
            policed_hosts: vec!["example.com".into()],
        };
        // No guard block: the config passes through untouched.
        let passthrough = narrow_guard_config(&config, None);
        assert_eq!(passthrough.checks, config.checks);
        // A def asking for secret_leak only: every other rule goes off —
        // including one the ADMIN left on by default — and the total map
        // spells the off rules explicitly rather than relying on defaults.
        config.checks.insert("pii_leak".into(), json!(false)); // admin already off
        let narrowed = narrow_guard_config(&config, Some(&["secret_leak"]));
        assert_eq!(narrowed.checks.get("secret_leak"), Some(&json!(true)));
        assert_eq!(narrowed.checks.get("pii_leak"), Some(&json!(false)));
        assert_eq!(narrowed.checks.get("ungrounded_ref"), Some(&json!(false)));
        // The def names it, the admin turned it off: off.
        config.checks.insert("secret_leak".into(), json!(false));
        let narrowed = narrow_guard_config(&config, Some(&["secret_leak"]));
        assert_eq!(narrowed.checks.get("secret_leak"), Some(&json!(false)));
        // Everything but the checks map rides along.
        assert_eq!(narrowed.policed_hosts, config.policed_hosts);
        assert_eq!(narrowed.mode, config.mode);
        assert_eq!(narrowed.min_confidence, config.min_confidence);
    }
}

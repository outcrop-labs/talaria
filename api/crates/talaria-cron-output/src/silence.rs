// HERMES'S "NOTHING TO REPORT", READ THE WAY HERMES READS IT.
//
// A cron prompt tells the agent (`cron/scheduler_prompt.py`): "If there is
// genuinely nothing new to report, respond with exactly "[SILENT]" (nothing
// else) to suppress delivery." Hermes honours it for its own delivery targets
// (`cron/scheduler.py`: `_is_cron_silence_response` on a successful run's
// final response, then "skipping delivery"), but neither of Talaria's roads
// passes through that check: the talaria-events plugin's `post_llm_call` hook
// fires on the final text before Hermes decides, and the pull reads the
// session's stored rows. Both used to land the sentinel itself, so a thread
// listening to a quiet daily check filled up with messages reading "[SILENT]".
//
// The rule is Hermes v0.22.4's own, ported exactly, so a run Hermes would not
// have delivered is never delivered here and a run it would have delivered
// always is (`gateway/response_filters.py`,
// `is_autonomous_silence_response`, which the cron lane shares with the
// webhook lane):
//
// - a marker is `[SILENT]`, `SILENT`, `NO_REPLY` or `NO REPLY`, or the zh-Hans
//   `[静默]`, `静默`, `[沉默]`, `沉默` (a lane that does not think in English
//   translates the sentinel rather than dropping it);
// - the response is silent when, whitespace-trimmed and upper-cased, it OPENS
//   with a bracketed marker (`[SILENT] nothing new since yesterday`), or when
//   the whole response, its first non-blank line or its last non-blank line
//   IS a marker;
// - "is a marker" compares at most 64 characters, upper-cased with whitespace
//   runs folded to one space, either as written or with edge punctuation
//   stripped (`.NO_REPLY`, `*NO_REPLY*`, `【静默】`) — square brackets are never
//   stripped, so a malformed `[SILENT` stays content;
// - a marker buried mid-sentence ("the job no longer prints [SILENT] when…")
//   is content and is delivered, and so is a bare `Silent retry succeeded`.
//
// Python's `str.strip`/`split`/`splitlines` and `str.upper` are mirrored
// below where Rust's built-ins differ (the information-separator controls
// U+001C–U+001F count as whitespace and line breaks in Python).

use std::sync::LazyLock;

use regex::Regex;

/// `LIVE_GATEWAY_SILENT_MARKERS`, canonical (upper-case, single-spaced).
const MARKERS: &[&str] = &[
    "[SILENT]", "SILENT", "NO_REPLY", "NO REPLY", "[静默]", "静默", "[沉默]", "沉默",
];

/// `_MARKER_LENGTH_CAP`: longer than any marker could plausibly be, even with
/// stray punctuation. Counted in characters, as Python's `len` counts.
const MARKER_LENGTH_CAP: usize = 64;

/// Python's `str.isspace`: Rust's White_Space plus the four information
/// separators Python also counts.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// Python's `str.splitlines()`: `\r\n` is one break, and so is each of the
/// other line boundaries Python recognises.
fn py_splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !boundary {
            continue;
        }
        lines.push(&s[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r'
            && let Some(&(j, '\n')) = chars.peek()
        {
            chars.next();
            next = j + 1;
        }
        start = next;
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// `_canonical_silence_candidate`: upper-cased, whitespace runs folded.
fn canonical(s: &str) -> String {
    py_strip(s)
        .to_uppercase()
        .split(py_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

static PUNCTUATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\p{P}$").expect("a fixed pattern compiles"));

/// `_is_edge_punctuation`: Unicode punctuation (any `P*` category) other
/// than the square brackets, which stay structural.
fn edge_punctuation(c: char) -> bool {
    if c == '[' || c == ']' {
        return false;
    }
    let mut buf = [0u8; 4];
    PUNCTUATION.is_match(c.encode_utf8(&mut buf))
}

/// `is_intentional_silence_response`: the text, as written or with its edge
/// punctuation stripped, IS a marker.
fn is_marker(text: &str) -> bool {
    let stripped = py_strip(text);
    let len = stripped.chars().count();
    if len == 0 || len > MARKER_LENGTH_CAP {
        return false;
    }
    let depunctuated = py_strip(stripped.trim_matches(edge_punctuation));
    [stripped, depunctuated]
        .into_iter()
        .any(|form| MARKERS.contains(&canonical(form).as_str()))
}

/// Should this cron report be delivered as nothing? Hermes's
/// `is_autonomous_silence_response` (see the header).
pub fn is_silence(response: &str) -> bool {
    let stripped = py_strip(response);
    if stripped.is_empty() {
        return false;
    }
    let upper = stripped.to_uppercase();
    if MARKERS
        .iter()
        .filter(|m| m.starts_with('['))
        .any(|m| upper.starts_with(m))
    {
        return true;
    }
    let lines: Vec<&str> = py_splitlines(stripped)
        .into_iter()
        .filter(|l| !py_strip(l).is_empty())
        .collect();
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
        return is_marker(stripped);
    };
    [stripped, first, last].into_iter().any(is_marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every case below is one Hermes v0.22.4's matcher decides the same way
    // (`gateway/response_filters.py`); the comments name the rule each one
    // pins.
    #[test]
    fn the_sentinel_alone_is_silence_however_it_is_dressed() {
        for text in [
            "[SILENT]",
            "  [SILENT]\n",
            "[silent]",
            "SILENT",
            "silent",
            "NO_REPLY",
            "no reply",
            "NO   REPLY",
            "No\treply",
            // Edge punctuation comes off; the marker is what is left.
            ".NO_REPLY",
            "*NO_REPLY*",
            "SILENT.",
            "\"[SILENT]\"",
            "...[SILENT]!",
            // The zh-Hans forms, bracketed by CJK punctuation too.
            "[静默]",
            "静默",
            "【静默】",
            "静默。",
            "[沉默]",
            "沉默",
        ] {
            assert!(is_silence(text), "{text:?} is silence");
        }
    }

    #[test]
    fn a_bracketed_sentinel_opening_a_short_note_is_silence() {
        for text in [
            "[SILENT] No changes detected",
            "[SILENT] nothing new since yesterday's digest.",
            "[silent] — quiet night",
            "[SILENT]\nChecked 14 feeds, nothing new.",
            "[静默] 没有新内容",
        ] {
            assert!(is_silence(text), "{text:?} is silence");
        }
    }

    #[test]
    fn a_sentinel_on_its_own_first_or_last_line_is_silence() {
        for text in [
            "Checked the inbox: nothing new.\n[SILENT]",
            "Checked the inbox: nothing new.\n\n  NO_REPLY  \n",
            "SILENT\nnothing changed overnight",
            "All quiet on every feed.\r\n[SILENT]",
            // Python's line boundaries, not only \n.
            "All quiet.\u{2028}[SILENT]",
            "All quiet.\u{1e}NO_REPLY",
        ] {
            assert!(is_silence(text), "{text:?} is silence");
        }
    }

    #[test]
    fn a_report_that_mentions_the_sentinel_is_delivered() {
        for text in [
            // Mid-sentence: content, not control.
            "The job no longer answers [SILENT] when the feed is empty.",
            "Morning digest: 3 new tickets. (Yesterday was [SILENT].)",
            // The bracketless prefix rule does not exist: a bare word at the
            // start of a sentence is content.
            "Silent retry succeeded on the second attempt.",
            "No reply from the vendor yet; I will check again tomorrow.",
            // A malformed bracket is never repaired into a marker.
            "[SILENT",
            "SILENT]",
            // A marker-sized line longer than the cap is content.
            &format!("{}SILENT", ".".repeat(70)),
            // The middle line of a three-line report is not an edge.
            "Digest:\n[SILENT]\nTwo new releases shipped.",
            // An ordinary report.
            "Overnight: 2 alerts, both resolved. Details below.",
        ] {
            assert!(!is_silence(text), "{text:?} is a report");
        }
    }

    #[test]
    fn blank_is_not_silence() {
        // Hermes routes empty output to its empty-response failure path, not
        // to silence; both roads already refuse to land a blank report.
        for text in ["", "   ", "\n\n", "\u{1c}"] {
            assert!(!is_silence(text), "{text:?}");
        }
    }

    #[test]
    fn python_line_and_space_rules() {
        assert_eq!(py_splitlines("a\r\nb\rc\nd"), vec!["a", "b", "c", "d"]);
        assert_eq!(py_splitlines("a\n"), vec!["a"]);
        assert_eq!(py_splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(py_splitlines("a\u{85}b\u{0b}c"), vec!["a", "b", "c"]);
        assert_eq!(py_strip("\u{1f} x \u{a0}"), "x");
        assert_eq!(canonical(" no \t  reply "), "NO REPLY");
        assert!(edge_punctuation('【') && edge_punctuation('。') && edge_punctuation('*'));
        assert!(!edge_punctuation('[') && !edge_punctuation(']') && !edge_punctuation('S'));
        // Symbols are not punctuation: Python's category check is P*, not S*.
        assert!(!edge_punctuation('$') && !edge_punctuation('+') && !edge_punctuation('~'));
    }
}

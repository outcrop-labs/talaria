// The one first-line extractor for TEXT harnesses, the way json.rs is the one
// extractor for structured ones — the port of harness/text.ts.
//
// WHY THIS MODULE EXISTS
//   `titler.ts` and `skill-summaries.ts` shipped the same eight lines — first
//   non-empty line, strip leading `["'#*\s]`, strip trailing quotes — written
//   twice with a one-character difference (the titler stripped a trailing `*`,
//   the summarizer did not). Two copies of one contract is how the audit's six
//   JSON extractors happened, and the port arrived with both copies intact.
//
//   Executing them against realistic 7-14B replies found two shapes that store
//   GARBAGE rather than failing, which is the worse of the two failure modes:
//   a failure keeps the previous title/summary, but a stored "```" persists
//   until the content hash changes.
//
//     "```\nWrites release notes.\n```"      stored the literal "```"
//     "**Writes release notes** from PRs."   stored a trailing "**" (summarizer)
//     "Here's the summary:\n\nWrites …"      stored the lead-in
//
//   All three are the model wrapping its answer rather than refusing to
//   answer, so the right response is to unwrap it, not to fail the contract
//   and leave a chat wearing its first message.
//
// PURE. No database, no gateway, no settings.
//
// THE PORT'S ONE MECHANICAL DIVERGENCE: `INLINE_MARKUP` in TS is a
// backreference regex (`/(\*\*|__|`)(.+?)\1/g`), and this house's regex
// crate has no backreferences — so the balanced-inline unwrap below is
// hand-rolled to the same left-to-right, lazy-closest-closer, at-least-one-
// inner-character contract the JS engine executes. The test corpus pins the
// shapes that matter, including the partial-bold case the TS file was
// written to fix.

use regex::Regex;
use std::sync::OnceLock;

/// A line that is nothing but a code-fence delimiter (``` or ```bash). It is
/// never the answer, and it was being stored AS the answer. JS `\w` is
/// ASCII-only — spelled out so a unicode-aware `\w` cannot widen it.
fn fence_only() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*`{3,}[0-9A-Za-z_-]*\s*$").unwrap())
}

/// A lead-in the model wrote before its real answer ("Here's the summary:").
/// Only skipped when something follows it — a one-line reply that happens to
/// end in a colon is that reply, and dropping it would fail a usable value.
fn lead_in(line: &str) -> bool {
    // `:\s*$` — a colon, then only whitespace to the end.
    let trimmed = line.trim_end();
    trimmed.ends_with(':')
}

/// The same thing WITHOUT the colon, which is how a small model most often
/// ignores "reply with ONLY the title": "Sure, here's a good title\n\nCheckout
/// Latency on Mobile". The colon form was skipped and this one was STORED — as
/// a conversation's name, as a skill's Studio subtitle — and because the
/// stored preamble no longer matches the mechanical title, neither retitle
/// sweep ever revisits the row.
///
/// ANCHORED OPENERS ONLY, and only when an answer follows. This is
/// deliberately a list of ways a model addresses the USER rather than a guess
/// at what a title looks like: "Based on the transcript" is never a title, but
/// "Checkout latency on mobile" must survive untouched, and a heuristic that
/// reaches for meaning would eventually eat one.
fn conversational_lead_in() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:sure|certainly|absolutely|of course|ok|okay|got it|understood|no problem|here (?:is|are)|here's|below (?:is|are)|based on|as requested|i(?:'ll| will| have| can)\b)",
        )
        .unwrap()
    })
}

/// A list marker the model put in front of a one-line answer. `*` is already
/// in the leading-decoration strip; `-`, `+`, `•` and `1.` / `1)` were not, so
/// a model that had been reading markdown all prompt named a chat
/// "- Checkout latency on mobile". The trailing space is required so a
/// negative number or an en-dash title is not a list.
fn list_marker() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(?:[-+•*]|\d{1,3}[.)])\s+").unwrap())
}

/// Balanced inline markup around part of the line, unwrapped BEFORE the
/// edge-decoration strip — on "**Checkout Latency** on Mobile" an
/// edge-only strip removes the opening `**` and leaves the closing one
/// mid-string, turning a symmetric shape anything could strip into an
/// asymmetric one nothing can. That partial-bold case is one of the three
/// this module's header says it was created to fix.
///
/// Hand-rolled (see the module header): scan left to right; an opener
/// (`**`, `__`, `` ` ``) with at least one inner character and a SAME
/// delimiter later unwraps to the inner text and resumes after the closer;
/// anything else advances one character. Lazy `.+?` = the CLOSEST closer.
fn unwrap_inline_markup(line: &str) -> String {
    const DELIMS: [&str; 3] = ["**", "__", "`"];
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    'outer: while !rest.is_empty() {
        for d in DELIMS {
            if !rest.starts_with(d) {
                continue;
            }
            if let Some(offset) = rest[d.len()..].find(d) {
                let inner_end = d.len() + offset;
                if inner_end > d.len() {
                    out.push_str(&rest[d.len()..inner_end]);
                    rest = &rest[inner_end + d.len()..];
                    continue 'outer;
                }
            }
        }
        let step = rest.chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&rest[..step]);
        rest = &rest[step..];
    }
    out
}

/// Decoration a model wraps a one-line answer in. Leading `#` is a heading
/// marker; it is deliberately NOT in the trailing class, because `#` at the
/// end of a line is content ("Sprint 14 planning #3") far more often than it
/// is a closing ATX marker.
fn decoration_leading(line: &str) -> &str {
    line.trim_start_matches(|c: char| {
        c == '"' || c == '\'' || c == '“' || c == '‘' || c == '#' || c == '*' || c.is_whitespace()
    })
}

fn decoration_trailing(line: &str) -> &str {
    line.trim_end_matches(|c: char| {
        c == '"' || c == '”' || c == '’' || c == '*' || c.is_whitespace()
    })
}

/// The first line of a reply that is actually the answer, undecorated — or
/// None when the reply carried nothing usable, which every caller reads as
/// "keep what you had".
pub fn first_meaningful_line(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw
        .split('\n')
        .filter(|l| !fence_only().is_match(l))
        .collect();
    for (i, raw_line) in lines.iter().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        // A lead-in only counts as one if there is an answer under it.
        let answer_follows = lines[i + 1..].iter().any(|l| !l.trim().is_empty());
        if answer_follows && (lead_in(line) || conversational_lead_in().is_match(line)) {
            continue;
        }
        let unmarked = list_marker().replace(line, "").into_owned();
        let unwrapped = unwrap_inline_markup(&unmarked);
        let stripped = decoration_trailing(decoration_leading(&unwrapped));
        if !stripped.is_empty() {
            return Some(stripped.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::first_meaningful_line as fml;

    // The corpus is the TS file's, transcribed — each case was a STORED
    // artifact once, which is why it is an assertion and not an anecdote.

    #[test]
    fn takes_the_bare_answer_unchanged() {
        assert_eq!(
            fml("Writes release notes from merged PRs."),
            Some("Writes release notes from merged PRs.".into())
        );
    }

    #[test]
    fn unwraps_a_fenced_answer_instead_of_storing_the_fence() {
        assert_eq!(
            fml("```\nWrites release notes from merged PRs.\n```"),
            Some("Writes release notes from merged PRs.".into())
        );
        assert_eq!(
            fml("```markdown\nSprint 14 planning\n```"),
            Some("Sprint 14 planning".into())
        );
    }

    #[test]
    fn strips_a_trailing_bold_marker_not_only_a_leading_one() {
        assert_eq!(
            fml("**Writes release notes**"),
            Some("Writes release notes".into())
        );
    }

    #[test]
    fn skips_a_lead_in_when_an_answer_follows_it() {
        assert_eq!(
            fml("Here's the summary:\n\nWrites release notes from merged PRs."),
            Some("Writes release notes from merged PRs.".into())
        );
    }

    #[test]
    fn keeps_a_one_line_answer_that_merely_ends_in_a_colon() {
        assert_eq!(
            fml("Deploys, in three steps:"),
            Some("Deploys, in three steps:".into())
        );
    }

    // The colon form was already skipped; this is the same preamble without
    // one, which is how a small model most often ignores "reply with ONLY the
    // title". Every one of these used to be STORED.
    #[test]
    fn skips_a_lead_in_that_does_not_end_in_a_colon() {
        let answer = "Checkout Latency on Mobile";
        for lead in [
            "Sure, here's a good title",
            "Understood. I will return the title.",
            "Here is the title.",
            "Okay, here is a concise title",
            "Based on the conversation, the title is",
            "Absolutely, here's a fitting title",
            "I will name it as follows.",
        ] {
            assert_eq!(
                fml(&format!("{lead}\n\n{answer}")),
                Some(answer.into()),
                "{lead}"
            );
        }
    }

    #[test]
    fn keeps_a_one_line_answer_that_merely_opens_with_one_of_those_words() {
        // Nothing follows, so it is the answer rather than a lead-in.
        assert_eq!(fml("Here is the plan"), Some("Here is the plan".into()));
        assert_eq!(
            fml("Based on the transcript"),
            Some("Based on the transcript".into())
        );
    }

    #[test]
    fn strips_a_list_marker_the_model_put_in_front_of_its_one_line_answer() {
        assert_eq!(
            fml("- Checkout latency on mobile"),
            Some("Checkout latency on mobile".into())
        );
        assert_eq!(
            fml("1. Checkout latency on mobile"),
            Some("Checkout latency on mobile".into())
        );
        assert_eq!(
            fml("2) Checkout latency on mobile"),
            Some("Checkout latency on mobile".into())
        );
        assert_eq!(
            fml("• Checkout latency on mobile"),
            Some("Checkout latency on mobile".into())
        );
        // Not a list: no space after the marker.
        assert_eq!(
            fml("-5 degrees and falling"),
            Some("-5 degrees and falling".into())
        );
    }

    #[test]
    fn unwraps_bold_around_part_of_the_line_instead_of_leaving_the_closing_marker() {
        // The half-stripped shape ("Checkout Latency** on Mobile") is worse
        // than no strip: it is asymmetric, so nothing downstream can remove it.
        assert_eq!(
            fml("**Checkout Latency** on Mobile"),
            Some("Checkout Latency on Mobile".into())
        );
        assert_eq!(
            fml("**Writes release notes** from PRs."),
            Some("Writes release notes from PRs.".into())
        );
        assert_eq!(
            fml("Reads `SKILL.md` and posts"),
            Some("Reads SKILL.md and posts".into())
        );
    }

    #[test]
    fn strips_quotes_and_heading_markers() {
        assert_eq!(
            fml("## \"Checkout latency on mobile\""),
            Some("Checkout latency on mobile".into())
        );
    }

    #[test]
    fn returns_none_when_nothing_survives() {
        assert_eq!(fml(""), None);
        assert_eq!(fml("\n\n   \n"), None);
        assert_eq!(fml("```\n```"), None);
        assert_eq!(fml("***"), None);
    }
}

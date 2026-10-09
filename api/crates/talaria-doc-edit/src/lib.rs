// DOCUMENT EDITING — the text surgery behind `edit_document`.
//
// WHY THIS EXISTS. `update_document` takes "New FULL markdown body", so every
// change an agent made to a document was a regeneration of the whole thing.
// That is three bad things at once: it is billed for the entire document on
// every tweak, it silently drops sections the model did not bother to re-emit,
// and it overwrites whatever a human typed into the same document while the
// agent was thinking. A surgical edit is none of those.
//
// WHY A LEAF CRATE WITH NO DEPENDENCIES. This is pure text, and the matching
// rules are the part that decides whether a model can use the tool at all — so
// they want to be iterated on against tests that compile in a second, not
// behind sqlx. The route reads the body, calls `apply`, and saves; nothing in
// here knows what a database is.
//
// THE CONTRACT A MODEL HAS TO HOLD, and why it is strict. `old` must match
// EXACTLY ONCE. Zero matches and many matches are both errors, and neither is
// softened into "do the first one" — a tool that guesses which paragraph you
// meant will eventually rewrite the wrong one, and the model has no way to
// find out. Every coding agent on the market already holds this contract for
// file edits, so models are well practised at it; the error text below is
// written to be read BY a model, because it is the model that has to recover.
//
// ALL OR NOTHING. Edits apply in sequence, each against the result of the one
// before — which is what lets a model fix two things in one call — but a
// failure anywhere discards the batch. A half-applied batch leaves a document
// in a state neither the model nor the person asked for, and the model's next
// read would show it changes it did not intend to keep.

pub mod wire;

/// One edit. `Replace` is the general form; `Section` exists because prose
/// rarely has a short unique anchor — quoting a whole paragraph exactly to
/// rewrite it is the kind of thing a model gets subtly wrong, and a heading is
/// an address it cannot typo invisibly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Replace the one occurrence of `old` with `new`. An empty `new` deletes.
    Replace { old: String, new: String },
    /// Replace the BODY under a markdown heading, keeping the heading line.
    ///
    /// The section runs from its heading to the next heading of the same or a
    /// shallower level, or to the end of the document. `heading` may carry its
    /// `#`s (`"## Rollback"` — then the level must match too) or be bare
    /// (`"Rollback"` — any level, as long as only one heading says it).
    Section { heading: String, markdown: String },
}

/// What one edit did, for the sentence the stream shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The section's heading, or a short quote of what was replaced.
    pub label: String,
    pub added: usize,
    pub removed: usize,
}

/// A successful batch. The body is the whole new document; nothing is written
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub body: String,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// No edits at all. A call that changes nothing is a mistake worth saying
    /// out loud rather than a no-op save that burns a version.
    Empty,
    /// `old` was not in the document.
    NotFound { index: usize, old: String },
    /// `old` was in the document more than once, so which one was meant is
    /// unknowable.
    Ambiguous {
        index: usize,
        old: String,
        count: usize,
    },
    /// An empty `old`, which "matches" everywhere and means nothing.
    EmptyNeedle { index: usize },
    SectionNotFound {
        index: usize,
        heading: String,
        /// The headings the document actually has. Carried so the refusal can
        /// NAME them: the recovery from a missed address is "which ones are
        /// there", and answering that in the error is one turn where making
        /// the agent re-read the whole document is two. Capped — a long
        /// document's full outline is not an error message.
        available: Vec<String>,
    },
    SectionAmbiguous {
        index: usize,
        heading: String,
        count: usize,
    },
}

/// How much of a quote to put in an error or a label. Long enough to identify
/// the text, short enough that a tool result is not a copy of the document.
const QUOTE: usize = 60;

/// A clipped, single-line rendering of a needle for error text and labels.
/// Newlines become `⏎` because a multi-line error message in a tool result
/// reads as several errors.
fn quote(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| match c {
            '\n' => '⏎',
            '\r' => ' ',
            '\t' => ' ',
            c => c,
        })
        .collect();
    // Char-wise, not byte-wise: the quote is for a person and a model to read,
    // and slicing a multi-byte char in half panics.
    if flat.chars().count() <= QUOTE {
        return flat;
    }
    let head: String = flat.chars().take(QUOTE).collect();
    format!("{head}…")
}

impl std::fmt::Display for EditError {
    /// WRITTEN FOR THE MODEL THAT HAS TO RECOVER. Each one names the rule it
    /// broke and the move that fixes it, because this string is the entire
    /// tool result the agent gets back — "edit failed" would leave it to guess
    /// between "I quoted it wrong" and "the document is not what I think".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Empty => write!(f, "no edits were given — nothing to do"),
            EditError::EmptyNeedle { index } => write!(
                f,
                "edit {}: oldString is empty. It has to be the text you want replaced",
                index + 1
            ),
            EditError::NotFound { index, old } => write!(
                f,
                "edit {}: oldString was not found: \"{}\". Read the document with get_document \
                 and quote the text exactly as it appears, including indentation and punctuation",
                index + 1,
                quote(old)
            ),
            EditError::Ambiguous { index, old, count } => write!(
                f,
                "edit {}: oldString appears {} times: \"{}\". It has to match exactly once — \
                 include more of the surrounding lines so only one place matches",
                index + 1,
                count,
                quote(old)
            ),
            EditError::SectionNotFound {
                index,
                heading,
                available,
            } => {
                write!(
                    f,
                    "edit {}: no heading matches \"{}\".",
                    index + 1,
                    quote(heading)
                )?;
                if available.is_empty() {
                    write!(
                        f,
                        " This document has no headings at all — use oldString/newString instead"
                    )
                } else {
                    write!(
                        f,
                        " The headings in this document are: {}. Use one of those, \
                         or oldString/newString instead",
                        available.join(" / ")
                    )
                }
            }
            EditError::SectionAmbiguous {
                index,
                heading,
                count,
            } => write!(
                f,
                "edit {}: {} headings say \"{}\". Give the heading with its #s to pick a level, \
                 or use oldString/newString with surrounding context",
                index + 1,
                count,
                quote(heading)
            ),
        }
    }
}

/// Apply a batch in order. Returns the whole new body, or the first failure
/// with nothing applied.
pub fn apply(body: &str, edits: &[Edit]) -> Result<Applied, EditError> {
    if edits.is_empty() {
        return Err(EditError::Empty);
    }
    let mut out = body.to_string();
    let mut changes = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let change = match edit {
            Edit::Replace { old, new } => replace_once(&mut out, index, old, new)?,
            Edit::Section { heading, markdown } => {
                replace_section(&mut out, index, heading, markdown)?
            }
        };
        changes.push(change);
    }
    Ok(Applied { body: out, changes })
}

/// THE CRLF RETRY, and why it is not a normalization. A document that came
/// from a Windows editor or a pasted email has `\r\n`; a model quoting it back
/// writes `\n`, and the literal match fails over a character nobody can see.
/// Rewriting the whole document's line endings to fix that would change every
/// line the edit did not touch — so instead the NEEDLE is expanded to the
/// document's own spelling, and only when the literal form matched nothing.
fn candidates(old: &str, haystack: &str) -> Vec<String> {
    let mut forms = vec![old.to_string()];
    if haystack.contains("\r\n") && !old.contains('\r') && old.contains('\n') {
        forms.push(old.replace('\n', "\r\n"));
    }
    forms
}

fn replace_once(
    body: &mut String,
    index: usize,
    old: &str,
    new: &str,
) -> Result<Change, EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyNeedle { index });
    }
    for form in candidates(old, body) {
        let count = body.matches(form.as_str()).count();
        if count == 1 {
            let at = body
                .find(form.as_str())
                .expect("one match was just counted");
            body.replace_range(at..at + form.len(), new);
            return Ok(Change {
                label: quote(old),
                added: new.len(),
                removed: form.len(),
            });
        }
        if count > 1 {
            return Err(EditError::Ambiguous {
                index,
                old: old.to_string(),
                count,
            });
        }
    }
    Err(EditError::NotFound {
        index,
        old: old.to_string(),
    })
}

/// A markdown ATX heading: the `#` run, then its text. Setext headings
/// (`Title\n=====`) are deliberately not addressed — nothing in Talaria writes
/// them, and a half-supported address is worse than one that says no.
struct Heading {
    level: usize,
    text: String,
    /// Byte range of the heading LINE, newline excluded.
    line: std::ops::Range<usize>,
}

fn headings(body: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut fenced = false;
    for line in body.split_inclusive('\n') {
        let len = line.len();
        let trimmed = line.trim_end_matches(['\n', '\r']);
        // A `#` inside a fenced code block is a comment in somebody's shell
        // snippet, not a section of the document.
        if trimmed.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let text = trimmed[level..].trim();
            // `#hashtag` is not a heading; ATX wants the space.
            if level <= 6 && (text.is_empty() || trimmed[level..].starts_with([' ', '\t'])) {
                out.push(Heading {
                    level,
                    text: text.to_string(),
                    line: at..at + trimmed.len(),
                });
            }
        }
        at += len;
    }
    out
}

/// The document's headings as a model should read them back — `## Rollback`,
/// with their levels, so the next attempt can address one exactly. Capped:
/// past a couple of dozen this stops being a hint and becomes the document.
const OUTLINE_CAP: usize = 24;

fn outline(all: &[Heading]) -> Vec<String> {
    all.iter()
        .take(OUTLINE_CAP)
        .map(|h| format!("{} {}", "#".repeat(h.level), h.text))
        .collect()
}

/// Split `"## Rollback"` into its level and text; a bare `"Rollback"` carries
/// no level and matches any.
fn parse_address(heading: &str) -> (Option<usize>, String) {
    let t = heading.trim();
    if !t.starts_with('#') {
        return (None, t.to_string());
    }
    let level = t.chars().take_while(|c| *c == '#').count();
    (Some(level), t[level..].trim().to_string())
}

fn replace_section(
    body: &mut String,
    index: usize,
    heading: &str,
    markdown: &str,
) -> Result<Change, EditError> {
    let (want_level, want_text) = parse_address(heading);
    let all = headings(body);
    let hits: Vec<&Heading> = all
        .iter()
        .filter(|h| {
            h.text.eq_ignore_ascii_case(&want_text) && want_level.is_none_or(|l| l == h.level)
        })
        .collect();
    let hit = match hits.len() {
        0 => {
            return Err(EditError::SectionNotFound {
                index,
                heading: heading.to_string(),
                available: outline(&all),
            });
        }
        1 => hits[0],
        count => {
            return Err(EditError::SectionAmbiguous {
                index,
                heading: heading.to_string(),
                count,
            });
        }
    };
    // THE SECTION ENDS AT THE NEXT HEADING THAT IS NOT INSIDE IT — same level
    // or shallower. A deeper heading (### under ##) is part of the section
    // being replaced, which is what makes "rewrite the Rollback section" mean
    // what a person means by it.
    let next = all
        .iter()
        .find(|h| h.line.start > hit.line.start && h.level <= hit.level);
    let end = next.map_or(body.len(), |h| h.line.start);
    // The heading LINE stays. The caller addressed the section by its name, so
    // renaming it here would be an edit they did not ask for — and a model
    // that wants a new name has oldString/newString for it.
    let after_heading = hit.line.end;
    let removed = end - after_heading;

    // A MODEL WILL OFTEN RE-EMIT THE HEADING, because it is writing the
    // section and the heading is the first line of a section. Taken
    // literally that produces the heading twice. Dropping an identical
    // leading heading is the one forgiveness here, and it is safe because it
    // only ever fires when the line it removes is the line that is already
    // there.
    let fresh = strip_leading_heading(markdown, hit.level, &hit.text);
    let body_text = fresh.trim_matches(['\n', '\r']);
    // THE SECTION'S RANGE RUNS TO THE START OF THE NEXT HEADING LINE, so it
    // swallowed the blank line that separated them — and the replacement has
    // to put it back, or the document grows a `text\n## Next` collision every
    // time a section is edited. At the end of the document there is no next
    // heading to stand off from, so one newline is the whole tail.
    let tail = if next.is_some() { "\n\n" } else { "\n" };
    let replacement = if body_text.is_empty() {
        tail.to_string()
    } else {
        format!("\n\n{body_text}{tail}")
    };
    body.replace_range(after_heading..end, &replacement);
    Ok(Change {
        label: format!("{} {}", "#".repeat(hit.level), hit.text),
        added: replacement.len(),
        removed,
    })
}

fn strip_leading_heading(markdown: &str, level: usize, text: &str) -> String {
    let trimmed = markdown.trim_start_matches(['\n', '\r']);
    let Some(first) = trimmed.split('\n').next() else {
        return markdown.to_string();
    };
    let line = first.trim_end_matches('\r');
    let (got_level, got_text) = parse_address(line);
    let same = got_level == Some(level) && got_text.eq_ignore_ascii_case(text);
    if !same {
        return markdown.to_string();
    }
    trimmed
        .split_once('\n')
        .map_or_else(String::new, |(_, rest)| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(old: &str, new: &str) -> Edit {
        Edit::Replace {
            old: old.into(),
            new: new.into(),
        }
    }
    fn section(heading: &str, markdown: &str) -> Edit {
        Edit::Section {
            heading: heading.into(),
            markdown: markdown.into(),
        }
    }

    #[test]
    fn replaces_the_one_occurrence_and_leaves_the_rest_byte_for_byte() {
        let doc = "# Plan\n\nShip on Tuesday.\n\nOwner: Dana\n";
        let out = apply(doc, &[replace("Tuesday", "Thursday")]).expect("one match");
        assert_eq!(out.body, "# Plan\n\nShip on Thursday.\n\nOwner: Dana\n");
        assert_eq!(out.changes.len(), 1);
    }

    #[test]
    fn an_empty_new_string_deletes() {
        let out = apply("keep\nDROP ME\nkeep\n", &[replace("DROP ME\n", "")]).expect("match");
        assert_eq!(out.body, "keep\nkeep\n");
    }

    #[test]
    fn edits_apply_in_sequence_so_the_second_sees_the_first() {
        // The point of a batch: a model fixes two things in one call, and the
        // second edit may quote text the first one wrote.
        let out = apply(
            "alpha\nbeta\n",
            &[replace("alpha", "gamma"), replace("gamma\nbeta", "done")],
        )
        .expect("both");
        assert_eq!(out.body, "done\n");
        assert_eq!(out.changes.len(), 2);
    }

    #[test]
    fn a_missing_old_string_refuses_and_says_how_to_recover() {
        let err = apply("hello\n", &[replace("goodbye", "x")]).expect_err("no match");
        assert_eq!(
            err,
            EditError::NotFound {
                index: 0,
                old: "goodbye".into()
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("edit 1"), "{msg}");
        assert!(msg.contains("get_document"), "{msg}");
    }

    #[test]
    fn an_ambiguous_old_string_refuses_with_the_count() {
        let err = apply("go\ngo\ngo\n", &[replace("go", "stop")]).expect_err("three");
        assert_eq!(
            err,
            EditError::Ambiguous {
                index: 0,
                old: "go".into(),
                count: 3
            }
        );
        assert!(err.to_string().contains("3 times"), "{err}");
    }

    #[test]
    fn a_failure_anywhere_discards_the_whole_batch() {
        // THE INVARIANT THAT MATTERS MOST. The first edit is perfectly good;
        // the second cannot be applied. The document must come back untouched
        // rather than half-edited — `apply` returns Err and the caller has
        // nothing to save.
        let doc = "alpha\nbeta\n";
        let err = apply(doc, &[replace("alpha", "gamma"), replace("nowhere", "x")])
            .expect_err("second fails");
        assert!(
            matches!(err, EditError::NotFound { index: 1, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn an_empty_batch_and_an_empty_needle_are_both_refused() {
        assert_eq!(apply("x", &[]).expect_err("empty"), EditError::Empty);
        assert_eq!(
            apply("x", &[replace("", "y")]).expect_err("empty needle"),
            EditError::EmptyNeedle { index: 0 }
        );
    }

    #[test]
    fn a_crlf_document_matches_a_needle_written_with_plain_newlines() {
        // The invisible-character failure this retry exists for: the model
        // quotes two lines it just read, with \n, against a \r\n document.
        let doc = "# Title\r\n\r\nfirst line\r\nsecond line\r\n";
        let out = apply(doc, &[replace("first line\nsecond line", "one line")])
            .expect("the needle is expanded to the document's own endings");
        assert_eq!(out.body, "# Title\r\n\r\none line\r\n");
    }

    #[test]
    fn a_crlf_retry_never_fires_when_the_literal_form_already_matches() {
        let doc = "a\nb\r\nc\n";
        let out = apply(doc, &[replace("a\nb", "A")]).expect("literal match wins");
        assert_eq!(out.body, "A\r\nc\n");
    }

    #[test]
    fn replaces_a_section_body_and_keeps_its_heading() {
        let doc = "# Doc\n\n## Rollback\n\nold steps\n\n## Owners\n\nDana\n";
        let out = apply(doc, &[section("## Rollback", "1. stop\n2. revert")]).expect("section");
        assert_eq!(
            out.body,
            "# Doc\n\n## Rollback\n\n1. stop\n2. revert\n\n## Owners\n\nDana\n"
        );
        assert_eq!(out.changes[0].label, "## Rollback");
    }

    #[test]
    fn a_section_swallows_its_deeper_headings_but_stops_at_a_sibling() {
        // "Rewrite the Rollback section" means the subsections too — and
        // stops dead at the next ## so it cannot eat the document's tail.
        let doc = "## A\n\none\n\n### A.1\n\ntwo\n\n## B\n\nthree\n";
        let out = apply(doc, &[section("## A", "replaced")]).expect("section");
        assert_eq!(out.body, "## A\n\nreplaced\n\n## B\n\nthree\n");
    }

    #[test]
    fn a_section_runs_to_the_end_when_nothing_follows_it() {
        let doc = "# Doc\n\n## Last\n\nold\n";
        let out = apply(doc, &[section("Last", "new")]).expect("section");
        assert_eq!(out.body, "# Doc\n\n## Last\n\nnew\n");
    }

    #[test]
    fn a_bare_heading_matches_any_level_and_a_hashed_one_pins_it() {
        let doc = "## Notes\n\nbody\n";
        assert!(apply(doc, &[section("Notes", "x")]).is_ok());
        assert!(apply(doc, &[section("## Notes", "x")]).is_ok());
        // The level is part of the address when it is given, so a wrong level
        // is a miss rather than a quiet match on the only "Notes" there is.
        let err = apply(doc, &[section("### Notes", "x")]).expect_err("level is pinned");
        assert!(matches!(err, EditError::SectionNotFound { .. }), "{err:?}");
    }

    #[test]
    fn a_missed_heading_names_the_headings_that_are_there() {
        // THE RECOVERY IS THE MESSAGE. A model that addressed the wrong
        // heading needs to know which ones exist; telling it to "read the
        // document" spends a turn to learn what we already had in hand.
        let doc = "# Doc\n\n## Backout plan\n\nx\n\n### Owners\n\ny\n";
        let err = apply(doc, &[section("Rollback", "new")]).expect_err("no such heading");
        let msg = err.to_string();
        assert!(msg.contains("## Backout plan"), "{msg}");
        assert!(msg.contains("### Owners"), "{msg}");
        assert!(msg.contains("# Doc"), "{msg}");

        // A document with no headings at all says THAT, rather than offering
        // an empty list and the same advice.
        let flat = apply("just prose\n", &[section("Anything", "x")])
            .expect_err("nothing to address")
            .to_string();
        assert!(flat.contains("no headings at all"), "{flat}");
        assert!(flat.contains("oldString"), "{flat}");
    }

    #[test]
    fn the_outline_in_a_refusal_is_capped() {
        // A hundred-heading document's full outline is not an error message.
        let doc: String = (1..=100).map(|i| format!("## H{i}\n\nx\n\n")).collect();
        let msg = apply(&doc, &[section("Missing", "x")])
            .expect_err("no such heading")
            .to_string();
        assert!(msg.contains("## H1"), "{msg}");
        assert!(!msg.contains("## H99"), "{msg}");
    }

    #[test]
    fn two_headings_with_the_same_name_refuse_rather_than_pick() {
        let doc = "## Notes\n\na\n\n## Notes\n\nb\n";
        let err = apply(doc, &[section("Notes", "x")]).expect_err("ambiguous");
        assert_eq!(
            err,
            EditError::SectionAmbiguous {
                index: 0,
                heading: "Notes".into(),
                count: 2
            }
        );
        // Levels disambiguate when they differ, which is the way out the
        // message offers.
        let mixed = "# Notes\n\na\n\n## Notes\n\nb\n";
        assert!(apply(mixed, &[section("## Notes", "x")]).is_ok());
    }

    #[test]
    fn a_re_emitted_heading_is_not_written_twice() {
        // The most likely model mistake on this tool: it is writing a
        // section, so it writes the heading first.
        let doc = "## Rollback\n\nold\n";
        let out =
            apply(doc, &[section("## Rollback", "## Rollback\n\nnew steps")]).expect("section");
        assert_eq!(out.body, "## Rollback\n\nnew steps\n");
    }

    #[test]
    fn a_different_leading_heading_is_kept_because_it_is_content() {
        // Only an IDENTICAL heading is dropped. A sub-heading the model wrote
        // on purpose is part of the new section.
        let doc = "## Rollback\n\nold\n";
        let out = apply(doc, &[section("## Rollback", "### Steps\n\nnew")]).expect("section");
        assert_eq!(out.body, "## Rollback\n\n### Steps\n\nnew\n");
    }

    #[test]
    fn a_hash_inside_a_fenced_block_is_not_a_heading() {
        // Otherwise a shell snippet's comment becomes an addressable section
        // and, worse, truncates the section it sits in.
        let doc = "## Setup\n\n```sh\n# Install\nbun i\n```\n\nmore\n\n## Next\n\nx\n";
        let err = apply(doc, &[section("Install", "y")]).expect_err("not a heading");
        assert!(matches!(err, EditError::SectionNotFound { .. }), "{err:?}");
        // And the fenced comment does not cut the section short either.
        let out = apply(doc, &[section("## Setup", "replaced")]).expect("section");
        assert_eq!(out.body, "## Setup\n\nreplaced\n\n## Next\n\nx\n");
    }

    #[test]
    fn a_hashtag_without_a_space_is_not_a_heading() {
        let doc = "# Real\n\n#notaheading\n\nbody\n";
        let err = apply(doc, &[section("notaheading", "x")]).expect_err("not ATX");
        assert!(matches!(err, EditError::SectionNotFound { .. }), "{err:?}");
    }

    #[test]
    fn emptying_a_section_leaves_the_heading_standing() {
        let doc = "## Gone\n\nold\n\n## Kept\n\nx\n";
        let out = apply(doc, &[section("## Gone", "")]).expect("section");
        assert_eq!(out.body, "## Gone\n\n## Kept\n\nx\n");
    }

    #[test]
    fn a_quote_in_an_error_is_clipped_and_single_line() {
        let long = "x".repeat(200);
        let err = apply("doc", &[replace(&long, "y")]).expect_err("no match");
        let msg = err.to_string();
        assert!(msg.contains('…'), "{msg}");
        assert!(!msg.contains('\n'), "{msg}");
        // And a multi-line needle does not turn one error into three lines.
        let multi = apply("doc", &[replace("a\nb\nc", "y")])
            .expect_err("no match")
            .to_string();
        assert!(!multi.contains('\n'), "{multi}");
        assert!(multi.contains('⏎'), "{multi}");
    }

    #[test]
    fn a_multibyte_needle_does_not_panic_when_it_is_clipped() {
        // `quote` used to slice bytes; a 60-byte boundary inside an em dash
        // panics, and the panic would surface as a 500 on a tool call.
        let long = "é—".repeat(100);
        let msg = apply("doc", &[replace(&long, "y")])
            .expect_err("no match")
            .to_string();
        assert!(msg.contains('…'), "{msg}");
    }

    #[test]
    fn changes_report_what_moved() {
        let out = apply("one two\n", &[replace("one", "three")]).expect("match");
        assert_eq!(out.changes[0].removed, 3);
        assert_eq!(out.changes[0].added, 5);
        assert_eq!(out.changes[0].label, "one");
    }
}

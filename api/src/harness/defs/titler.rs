// The Titler, declared. Names things as they take shape: chats and plans after
// their first exchange, research runs from their question.
//
// This is the smallest harness in the product: everything around the ask —
// model resolution, failure handling, metering — is the runner's, and what is
// left below is the three prompts, the output contract, and the floor.
//
// THE FLOOR IS EMPTY ON PURPOSE. Naming a chat is the most forgiving job in
// Talaria: the reply is one short line, nothing downstream parses it, and a
// mediocre title is strictly better than the mechanical first-message
// truncation it replaces. A titler that REFUSED to run on an unprobed 7B model
// would leave every conversation wearing its truncation forever, which is a
// worse product than a slightly clumsy name. `requires` still says what the job
// actually leans on — following "reply with ONLY the title" — so the fitness
// suite can score it and an admin can see the weakness, but nothing here blocks.
//
// THE FIXTURES ARE THE FITNESS SUITE'S ROW, held here as a table. The check
// closes over the SAME input the model was given, so the restatement assertion
// is measured against the real transcript rather than a copy that can drift
// away from it.

use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::body::utf16_len;
use crate::harness::define::{
    AnswerFloor, CheckCtx, CheckResult, CountLimit, EvalBand, EvalCase, GuardDecl,
    HarnessDefinition, Message, OnFailure, Output, RenderContext, RoleFloor, below_answer_floor,
    count_problem, define_harness,
};
use crate::harness::text::first_meaningful_line;
use crate::harness_model::ModelSpec;

// ── The input ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleKind {
    Chat,
    Plan,
    Research,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitlerInput {
    pub kind: TitleKind,
    /// A transcript for chat/plan, the question itself for research.
    pub text: String,
}

// ── The prompts ──────────────────────────────────────────────────────────────

// Three jobs that share a shape and not a wording. A chat is named for its
// subject, a plan for what it delivers, a research run for what it
// investigates — and the research prompt has to say "not a question" out
// loud, because the input IS a question and a small model will otherwise
// hand it straight back.
//
// WHY THE RULES ARE SHARED. `title_problem` rejects a generic lead-in for all
// three kinds — one function, one `GENERIC_LEAD_IN` — so every kind has to be
// told the rule. Grading a `plan` or `research` title against a rule its
// prompt never stated measures our prompt rather than the model: a suite is
// only a measurement of the platform if it grades what the platform actually
// asks for.
const TITLE_RULES: &str = "No quotes, no trailing punctuation, never a generic filler opening like \"Chat about\" or \"Discussion of\". Reply with ONLY the title.";

pub fn prompt_for(kind: TitleKind) -> String {
    match kind {
        TitleKind::Chat => format!(
            "Name this conversation. 3–7 words, specific to what it is actually about — the subject, not the activity. {TITLE_RULES}"
        ),
        TitleKind::Plan => format!(
            "Name this plan. 3–7 words, outcome-focused — what the plan will deliver, not the conversation around it. {TITLE_RULES}"
        ),
        TitleKind::Research => format!(
            "Name this research run from its question. 3–7 words capturing the subject under investigation. Do not restate it as a question. {TITLE_RULES}"
        ),
    }
}

// ── The narrowing ────────────────────────────────────────────────────────────

/// Naming is worth one short call and no more. A transcript longer than this
/// says nothing about its own subject that the first few thousand characters
/// did not, and paying to send it would make the cheapest harness in the
/// product the most expensive one on a long chat.
fn clip(s: &str, max: usize) -> &str {
    if utf16_len(s) > max {
        crate::body::truncate_utf16(s, max)
    } else {
        s
    }
}

/// Raw reply -> a title, or None to fail the contract (the caller then keeps
/// the title it already had).
///
/// The unwrapping half is `first_meaningful_line` (harness/text.rs), the
/// shared text-harness extractor. What stays here is what is TRUE OF TITLES
/// and of nothing else:
///   - the trailing period, including the ideographic '。': the prompt says no
///     trailing punctuation and models add one anyway.
///   - the 90-character clamp with an ellipsis: a title is rendered in a sidebar
///     row, and a model that ignored "3-7 words" must not be able to push a
///     paragraph into that row.
///
/// The one thing it does NOT do is reject a long or generic title — that is the
/// fitness suite's job (see `fixtures`), not a reason to leave a chat unnamed.
pub fn clean_title(raw: &str) -> Option<String> {
    let line = first_meaningful_line(raw)?;
    // ONE trailing character is removed, never a run of them — "Sprint 14
    // planning.." keeps its second dot, and the fitness rule (not the
    // narrowing) is what flags it.
    let stripped = match line.strip_suffix('。') {
        Some(s) => s,
        None => line.strip_suffix('.').unwrap_or(&line),
    };
    let t = stripped.trim();
    if t.is_empty() {
        return None;
    }
    if utf16_len(t) > 90 {
        Some(format!(
            "{}…",
            crate::body::truncate_utf16(t, 90).trim_end()
        ))
    } else {
        Some(t.to_string())
    }
}

// ── Eval assertions ──────────────────────────────────────────────────────────
// Deterministic string facts, no judge model. These are the titler's row in the
// fitness matrix, and they are the reason a candidate model can be rejected for
// this job in seconds rather than after a week of oddly-named conversations.

fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

fn non_alnum_run() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap())
}

fn norm(s: &str) -> String {
    non_alnum_run()
        .replace_all(&s.to_lowercase(), " ")
        .trim()
        .to_string()
}

/// The fillers the chat prompt names, plus their obvious siblings. Kept to
/// CONVERSATION nouns: "Analysis of index bloat" is a perfectly good title and a
/// wider list would fail a model for writing one.
fn generic_lead_in() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:(?:a|an|the)\s+)?(?:chat|conversation|discussion|dialogue|thread|talk|exchange|session|notes?)\s+(?:about|on|of|regarding|concerning|with|between|for)\b",
        )
        .unwrap()
    })
}

fn speaker_prefix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^\s*(?:user|assistant|system)\s*:\s*").unwrap())
}

fn quote_wrapped() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"^["'“‘]|["'”’]$"#).unwrap())
}

fn ends_in_punctuation() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[.。!?]$").unwrap())
}

/// Did the model just hand the input back? Compared line by line with the
/// speaker prefix removed, because a chat transcript arrives as "user: ..." and
/// a restatement of the first message is the classic small-model answer. Short
/// titles are exempt: a three-word title that happens to open a sentence is a
/// coincidence, not a restatement.
fn restates_input(title: &str, text: &str) -> bool {
    let t = norm(title);
    // `norm` output is ASCII, so UTF-16 units and characters agree.
    if t.chars().count() < 12 {
        return false;
    }
    text.split('\n').any(|line| {
        let stripped = speaker_prefix().replace(line, "");
        norm(&stripped).starts_with(&t)
    })
}

/// One line naming what is wrong with this title, or None. This is what an
/// admin reads in the fitness drill-down, so it says the observed fact rather
/// than the rule id.
pub fn title_problem(title: &str, input: &TitlerInput, mentions: &[&str]) -> Option<String> {
    // THE ANSWER FLOOR: "is this an answer at all". It cannot live in the word
    // count — `{"nope": true}` is two words, inside any margin a count can
    // give. The floor and the count are different assertions and say so
    // separately: this one is "is this an answer at all", the one below is
    // "is it title-shaped".
    let floor = AnswerFloor {
        min_chars: 8,
        mentions: mentions.iter().map(|s| (*s).to_string()).collect(),
    };
    if let Some(thin) = below_answer_floor(title, &floor) {
        return Some(thin);
    }
    let units = utf16_len(title);
    if units > 90 {
        return Some(format!(
            "{units} characters — a title has to fit one sidebar row"
        ));
    }
    // A MARGIN, because "3–7 words" is a stated preference and the sidebar row is
    // the only hard edge — and that is a CHARACTER clamp (`clean_title` truncates
    // at 90 with an ellipsis), not a word count. An eight-word title renders
    // exactly as well as a seven-word one, and failing it measured how literally
    // the model read a range rather than whether it can name a conversation.
    let n = words(title).len() as i64;
    let limit = CountLimit {
        min: Some(3),
        max: Some(7),
        unit: "word",
        asked: "3-7",
        tolerance: None,
    };
    if let Some(over) = count_problem(n, &limit) {
        return Some(over);
    }
    if quote_wrapped().is_match(title) {
        return Some("wrapped in quotes".into());
    }
    if ends_in_punctuation().is_match(title) {
        return Some("ends in punctuation".into());
    }
    if generic_lead_in().is_match(title) {
        return Some("opens with a generic filler (\"Chat about\", \"Discussion of\")".into());
    }
    if input.kind == TitleKind::Research && title.contains('?') {
        return Some("restated the question instead of naming the subject".into());
    }
    if restates_input(title, &input.text) {
        return Some("restates the input verbatim instead of naming it".into());
    }
    None
}

// ── The fixtures ─────────────────────────────────────────────────────────────

/// One fixture. The check closes over the SAME input the model was given, so
/// the restatement assertion is measured against the real transcript rather
/// than a copy that can drift away from it.
///
/// `mentions` is the floor half, and it is per fixture because only the fixture
/// knows what its transcript is unmistakably ABOUT. It is a set of alternatives,
/// never a phrase: it has to reject a non-answer without scoring the model's
/// word choice, and a fixture only one wording can pass measures our prompt
/// rather than the model. See `below_answer_floor`.
pub struct TitleFixture {
    pub name: &'static str,
    pub band: EvalBand,
    pub input: TitlerInput,
    pub mentions: &'static [&'static str],
}

impl TitleFixture {
    /// The fixture's check: one line naming what is wrong with this title, or
    /// None when the model did the job.
    pub fn check(&self, value: &str) -> Option<String> {
        title_problem(value, &self.input, self.mentions)
    }
}

/// TEN FIXTURES ACROSS THREE BANDS. EASY is one obvious subject stated plainly
/// — a model that cannot name that cannot title anything. STANDARD is the job
/// as it arrives: a real transcript with more than one noun in it. HARD is
/// where the transcript actively misleads — a loud opening line that is not the
/// subject, a quotable phrase that is a trap, a question short enough that
/// handing it back looks like an answer.
pub fn fixtures() -> Vec<TitleFixture> {
    vec![
        TitleFixture {
            name: "chat — one plain subject, stated outright",
            band: EvalBand::Easy,
            input: TitlerInput {
                kind: TitleKind::Chat,
                text: [
                    "user: the nightly backup job has been failing since Tuesday",
                    "assistant: the target volume filled up — the retention sweep stopped running when the cron user lost write access.",
                ]
                .join("\n"),
            },
            mentions: &["backup", "retention", "volume", "cron", "disk"],
        },
        TitleFixture {
            name: "research — one plain subject, stated outright",
            band: EvalBand::Easy,
            input: TitlerInput {
                kind: TitleKind::Research,
                text: "How do European data residency rules apply to customer support transcripts?"
                        .to_string(),
            },
            mentions: &["residency", "data", "transcript", "support", "europe", "gdpr"],
        },
        TitleFixture {
            name: "plan — one plain deliverable, stated outright",
            band: EvalBand::Easy,
            input: TitlerInput {
                kind: TitleKind::Plan,
                text: [
                    "user: we need SSO working for the enterprise trial next month",
                    "assistant: that is SAML for two identity providers plus a group-to-role mapping.",
                ]
                .join("\n"),
            },
            mentions: &["sso", "saml", "identity", "sign-on", "login"],
        },
        TitleFixture {
            name: "chat — names the subject, not the activity",
            band: EvalBand::Standard,
            input: TitlerInput {
                kind: TitleKind::Chat,
                text: [
                    "user: our checkout page takes about nine seconds to load on mobile and people are dropping off at payment",
                    "assistant: the largest contentful paint is dominated by the payment iframe — it blocks render until the provider script resolves.",
                ]
                .join("\n"),
            },
            mentions: &[
                "checkout",
                "payment",
                "mobile",
                "load",
                "paint",
                "latency",
                "performance",
            ],
        },
        TitleFixture {
            name: "chat — does not adopt a quoted phrase from the transcript",
            band: EvalBand::Standard,
            input: TitlerInput {
                kind: TitleKind::Chat,
                text: [
                    "user: someone filed a ticket called \"URGENT!!! everything is broken.\" can we work out what they actually mean",
                    "assistant: the attached log shows a single failing migration on the reporting replica.",
                ]
                .join("\n"),
            },
            mentions: &["migration", "replica", "reporting", "ticket", "log"],
        },
        TitleFixture {
            name: "plan — names the outcome, not the conversation",
            band: EvalBand::Standard,
            input: TitlerInput {
                kind: TitleKind::Plan,
                text: [
                    "user: we need to get the warehouse off the old label printer before the holiday rush",
                    "assistant: that means new firmware on twelve stations, a template migration, and a fallback for the two sites still on serial.",
                ]
                .join("\n"),
            },
            mentions: &["printer", "label", "warehouse", "firmware", "station"],
        },
        TitleFixture {
            name: "research — names the subject without restating the question",
            band: EvalBand::Standard,
            input: TitlerInput {
                kind: TitleKind::Research,
                text: "What are the practical tradeoffs between Postgres logical replication and Debezium for feeding a warehouse in near real time?"
                        .to_string(),
            },
            mentions: &["replication", "debezium", "postgres", "warehouse", "cdc"],
        },
        // THE BURIED SUBJECT. The loudest line is the opening complaint and it is
        // not what the conversation turns out to be about. A weaker model titles
        // the first sentence it read.
        TitleFixture {
            name: "chat — the subject is not the opening line",
            band: EvalBand::Hard,
            input: TitlerInput {
                kind: TitleKind::Chat,
                text: [
                    "user: I am so tired of this deploy pipeline, it is genuinely the worst part of my week, every single time",
                    "assistant: which step is failing for you?",
                    "user: honestly the pipeline is fine. what keeps biting me is that staging and production have different Postgres extensions installed, so migrations pass in one and fail in the other.",
                    "assistant: so the real problem is extension drift between environments.",
                ]
                .join("\n"),
            },
            mentions: &[
                "extension",
                "drift",
                "postgres",
                "migration",
                "staging",
                "environment",
            ],
        },
        // A QUESTION SHORT ENOUGH TO HAND BACK. The prompt says "do not restate it
        // as a question"; the cheap move is to strip the question mark and return
        // the same words, which `restates_input` catches.
        TitleFixture {
            name: "research — a short question it must not simply hand back",
            band: EvalBand::Hard,
            input: TitlerInput {
                kind: TitleKind::Research,
                text: "Is Redis Streams a good fit for our job queue?"
                        .to_string(),
            },
            mentions: &["redis", "stream", "queue", "job"],
        },
        // A CONVERSATION ABOUT CONVERSATIONS. Every generic filler the prompt bans
        // is sitting right there in the transcript for the taking.
        TitleFixture {
            name: "chat — a meta subject, with every filler word available to steal",
            band: EvalBand::Hard,
            input: TitlerInput {
                kind: TitleKind::Chat,
                text: [
                    "user: we keep having the same discussion about how we run these meetings and nothing changes",
                    "assistant: what usually derails it?",
                    "user: nobody writes down the decision, so the next meeting relitigates it. we need a decision log.",
                ]
                .join("\n"),
            },
            mentions: &["decision", "log", "meeting", "record"],
        },
    ]
}

// ── The def ──────────────────────────────────────────────────────────────────

pub fn titler_harness() -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        "titler",
        "Titler",
        "Names things as they take shape: chats and plans after their first exchange, research runs from their question.",
        // The Titler pin from Models → Platform, then the Utility role, then
        // the env default, then the first routable model — the DEFAULT chain,
        // one implementation (harness_model.rs). No `role` field, because the
        // DEFAULT chain already carries a 'utility' step. Declaring
        // `role: "utility"` as well would win one step earlier and record
        // `harness_runs.chain_step = 'role'` for the same resolved model, so
        // two harnesses resolving identically would report different steps to
        // the fitness page. 'utility' means "the Utility role model carried
        // this" everywhere; 'role' is reserved for a harness that has a role
        // of its own.
        ModelSpec {
            pin: Some("titler"),
            role: None,
            chain: None,
            user_id: None,
        },
        Arc::new(|input: &Value, _ctx: &RenderContext| {
            let titler: TitlerInput =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            Ok(vec![
                Message::system(prompt_for(titler.kind)),
                Message::user(clip(&titler.text, 4000)),
            ])
        }),
        Output::Text {
            clean: Some(Arc::new(
                |raw: &str| Ok(clean_title(raw).map(Value::String)),
            )),
            verify: None,
        },
        // Fire and forget, and that is a product decision rather than laziness:
        // every caller of this harness already HAS a title (the mechanical
        // truncation, or nothing at all for a research run) and null means "keep
        // it". sweep_titles additionally reads null as a stop signal so a dead
        // model cannot burn a whole batch. Anything other than Null here would
        // break both.
        OnFailure::Null,
    ));
    // Not a floor — a fact for the fitness matrix. "Reply with ONLY the title"
    // is the whole contract, and a model that answers it with a paragraph is a
    // model whose titles get clamped to 90 characters of preamble. Worth
    // measuring, never worth refusing over.
    d.requires = vec!["instruction-following"];
    d.floor = RoleFloor::runs_anyway(
        "Runs on any model. A weak one writes clumsier names; nothing downstream reads a title, and a clumsy name beats leaving every chat wearing its first message.",
    );
    // Only the two rules that can fire on a five-word title, and the omission
    // matters more than the inclusion: `zero_tool_claim` reads "Deleted the
    // stale billing rows" as a claim of completed work with no tool behind it,
    // which for a TITLE is a false positive by construction — and
    // guard_findings per model is the live confabulation rate the fitness page
    // shows, so a titler filing a finding on every past-tense name would libel
    // the model it runs on. Titles are persisted and rendered in a sidebar, so
    // a credential that reached one gets scrubbed before it is stored.
    d.guard = Some(GuardDecl {
        rules: Some(vec!["secret_leak", "pii_leak"]),
        redact: true,
    });
    d.temperature = Some(0.3);
    // No `widen`. A frontier model already writes a better title from this
    // exact prompt; there is no MORE for it to do here, and the only thing a
    // wider prompt would buy is a longer name — which is the failure mode, not
    // the upgrade. Widening exists for depth and authority, and a title has
    // neither.

    // THE FIXTURE TABLE, folded onto the fitness plane's `EvalCase`. Each row
    // keeps its own floor terms and its own transcript (see `fixtures`); the
    // fold only re-types the value — a text harness's reply arrives as a JSON
    // string, and a value that is not one is the fixture check failing on it,
    // which the sweep scores as a task failure.
    // No `dry_run`: a titler turn calls no tools, so a replay of these rows
    // runs single-shot against the empty context.
    d.evals = fixtures()
        .into_iter()
        .map(|f| {
            let band = f.band;
            let input = serde_json::to_value(&f.input).expect("a fixture input serializes");
            EvalCase::new(
                f.name,
                input,
                Arc::new(move |v: &Value, _ctx: &CheckCtx| {
                    match serde_json::from_value::<String>(v.clone()) {
                        Ok(s) => f.check(&s).into(),
                        Err(e) => {
                            CheckResult::Fail(format!("the fixture check threw on the value: {e}"))
                        }
                    }
                }),
            )
            .band(band)
        })
        .collect();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::recorded::{
        RecordedModel as ModelAnswer, RecordedRun as Recorder, RecordedWorld as World, facts,
        probe, recorded_run, replies,
    };
    use crate::harness::run::{HarnessResult, RunContext, execute};

    // ── clean_title ──────────────────────────────────────────────────────────

    #[test]
    fn cleans_the_bare_answer() {
        assert_eq!(
            clean_title("Checkout latency on mobile"),
            Some("Checkout latency on mobile".into())
        );
        // The unwrapping half is first_meaningful_line's, not repeated here.
        assert_eq!(
            clean_title("Sure, here's a good title\n\nCheckout latency on mobile"),
            Some("Checkout latency on mobile".into())
        );
    }

    #[test]
    fn strips_one_trailing_period_including_the_ideographic_one() {
        assert_eq!(
            clean_title("Writes release notes."),
            Some("Writes release notes".into())
        );
        assert_eq!(clean_title("移行の drift"), Some("移行の drift".into()));
        assert_eq!(clean_title("移行の drift。"), Some("移行の drift".into()));
        // ONE character, not a run: `replace(/[.。]$/, '')` and nothing more.
        assert_eq!(
            clean_title("Sprint 14 planning.."),
            Some("Sprint 14 planning.".into())
        );
        // A period that is not last is content.
        assert_eq!(clean_title("v1.2 rollout"), Some("v1.2 rollout".into()));
    }

    #[test]
    fn clamps_at_ninety_utf16_units_with_an_ellipsis() {
        let eighty = "a".repeat(90);
        assert_eq!(clean_title(&eighty), Some(eighty.clone()));
        // 100 units -> 100 ASCII characters, cut to 90 plus the ellipsis.
        let long = "a".repeat(100);
        let clamped = clean_title(&long).unwrap();
        assert_eq!(utf16_len(&clamped), 91);
        assert!(clamped.ends_with('…'));
        // Trailing whitespace left by the cut is trimmed BEFORE the ellipsis.
        let padded = format!("{}{}", "a".repeat(89), "   bbbb");
        let clamped = clean_title(&padded).unwrap();
        assert_eq!(clamped, format!("{}…", "a".repeat(89)));
        // UTF-16, not characters: 45 astral characters are 90 units and pass
        // untouched; 46 are 92 and cut at the pair boundary.
        let emoji: String = "😀".repeat(45);
        assert_eq!(clean_title(&emoji), Some(emoji.clone()));
        let emoji46: String = "😀".repeat(46);
        assert_eq!(clean_title(&emoji46), Some(format!("{emoji}…")));
    }

    #[test]
    fn fails_the_contract_when_nothing_survives() {
        assert_eq!(clean_title(""), None);
        assert_eq!(clean_title("\n\n```\n```\n"), None);
        assert_eq!(clean_title("."), None);
    }

    // ── title_problem ────────────────────────────────────────────────────────

    fn problem(title: &str, kind: TitleKind, text: &str, mentions: &[&str]) -> Option<String> {
        title_problem(
            title,
            &TitlerInput {
                kind,
                text: text.into(),
            },
            mentions,
        )
    }

    #[test]
    fn the_answer_floor_rejects_the_non_answer_the_word_minimum_used_to_catch() {
        // The literal non-answer: fourteen characters, two words, mentions
        // none — inside any word-count margin, so only the floor catches it.
        let hit = problem(
            "{\"nope\": true}",
            TitleKind::Chat,
            "user: the nightly backup job has been failing since Tuesday",
            &["backup", "retention"],
        )
        .unwrap();
        assert!(hit.contains("it mentions none of [\"backup\",\"retention\"]"));
        // Too short to be an answer at all.
        let thin = problem("a title", TitleKind::Chat, "irrelevant", &[]).unwrap();
        assert!(thin.starts_with("the answer is 7 characters"));
    }

    #[test]
    fn a_long_title_is_a_sidebar_row_problem_not_a_word_problem() {
        // 95 one-letter words: 189 characters, so the CHARACTER clamp fires
        // first and says the row, not the count.
        let long = vec!["a"; 95].join(" ");
        assert_eq!(
            problem(&long, TitleKind::Chat, "irrelevant", &[]).as_deref(),
            Some("189 characters — a title has to fit one sidebar row")
        );
        // The count with its margin: eight words render exactly as well as
        // seven, so nothing is wrong with them.
        let eight = "One two three four five six seven eight";
        assert!(problem(eight, TitleKind::Chat, "irrelevant", &[]).is_none());
        // One word, and the paragraph, are different KINDS of answer. (The word
        // has to clear the answer floor first — "Deploys" is seven characters
        // and the floor would fire before the count ever got to speak.)
        assert_eq!(
            problem("Deployment", TitleKind::Chat, "irrelevant", &[]).as_deref(),
            Some("1 word — the prompt asks for 3-7")
        );
    }

    #[test]
    fn rejects_the_wrapping_a_model_adds_to_a_one_line_answer() {
        for title in [
            "\"Checkout latency\"",
            "'Checkout latency'",
            "Checkout latency\"",
            "“Checkout latency”",
        ] {
            assert_eq!(
                problem(title, TitleKind::Chat, "irrelevant", &[]).as_deref(),
                Some("wrapped in quotes"),
                "{title}"
            );
        }
        for title in ["Checkout latency.", "Checkout latency!"] {
            assert_eq!(
                problem(title, TitleKind::Chat, "irrelevant", &[]).as_deref(),
                Some("ends in punctuation"),
                "{title}"
            );
        }
        // A question mark that is not the LAST character is checked by the
        // research rule instead — and only there: on chat it is content.
        assert_eq!(
            problem(
                "Why? Redis Streams for queues",
                TitleKind::Research,
                "Is Redis Streams a good fit for our job queue?",
                &["redis", "stream", "queue"]
            )
            .as_deref(),
            Some("restated the question instead of naming the subject")
        );
        assert!(
            problem(
                "Why? Redis Streams for queues",
                TitleKind::Chat,
                "irrelevant",
                &[]
            )
            .is_none()
        );
    }

    #[test]
    fn the_generic_lead_in_list_stays_conversation_nouns() {
        for title in [
            "Chat about the backup failures",
            "A conversation on SSO",
            "The discussion of index bloat",
            "Notes on the deploy pipeline",
            "Thread regarding checkout latency",
        ] {
            assert_eq!(
                problem(title, TitleKind::Chat, "irrelevant", &[]).as_deref(),
                Some("opens with a generic filler (\"Chat about\", \"Discussion of\")"),
                "{title}"
            );
        }
        // "Analysis" is not a conversation noun: a wider list would fail a model
        // for writing a perfectly good title.
        assert!(
            problem(
                "Analysis of index bloat",
                TitleKind::Chat,
                "irrelevant",
                &[]
            )
            .is_none()
        );
    }

    #[test]
    fn catches_the_restatement_of_a_transcript_line() {
        // Handing the user's first message back, speaker prefix and all.
        let text = "user: our checkout page takes about nine seconds to load on mobile\nassistant: the payment iframe blocks render.";
        assert_eq!(
            problem(
                "Our checkout page takes about nine seconds",
                TitleKind::Chat,
                text,
                &["checkout", "payment"]
            )
            .as_deref(),
            Some("restates the input verbatim instead of naming it")
        );
        // The prefix is stripped before comparing, so restating the assistant's
        // line is caught by the same fold.
        assert_eq!(
            problem(
                "The payment iframe blocks render",
                TitleKind::Chat,
                text,
                &["checkout", "payment"]
            )
            .as_deref(),
            Some("restates the input verbatim instead of naming it")
        );
        // A short title that happens to open a sentence is exempt: "the payment"
        // is eleven normalized characters, under the twelve-character bar, so
        // even though the assistant's line opens with exactly those words this
        // is a coincidence and not a restatement.
        assert!(
            problem(
                "The payment",
                TitleKind::Chat,
                text,
                &["checkout", "payment"]
            )
            .is_none()
        );
    }

    // ── The fixtures, asserted the way the fitness sweep will assert them ────

    #[test]
    fn every_fixture_passes_a_title_that_names_its_subject() {
        let good = [
            "Nightly backup failures",
            "EU data residency for support transcripts",
            "SSO for the enterprise trial",
            "Mobile checkout latency at payment",
            "Reporting replica migration failure",
            "Label printer migration before the rush",
            "Postgres replication versus Debezium",
            "Postgres extension drift between staging and production",
            "Redis Streams for job queues",
            "A decision log for recurring meetings",
        ];
        for (fixture, title) in fixtures().iter().zip(good) {
            assert!(
                fixture.check(title).is_none(),
                "{}: {} failed on {:?}",
                fixture.name,
                title,
                fixture.check(title)
            );
        }
    }

    #[test]
    fn every_fixture_rejects_the_non_answer() {
        for fixture in fixtures() {
            assert!(
                fixture.check("{\"nope\": true}").is_some(),
                "{} accepted the literal non-answer",
                fixture.name
            );
        }
    }

    #[test]
    fn the_hard_band_fixtures_catch_their_named_traps() {
        // Fixed indices into the table (removal shifts — the three hard cases
        // are its last three entries).
        let fixtures = fixtures();
        // The buried subject: titling the loud opening line engages with none of
        // what the conversation is about.
        let buried = &fixtures[7];
        assert_eq!(buried.name, "chat — the subject is not the opening line");
        assert!(
            buried
                .check("The worst part of my week")
                .unwrap()
                .contains("mentions none of")
        );
        // The short question handed back: same words, question mark dropped.
        // (The full sentence handed back would be caught a step earlier by the
        // word count; seven words is inside the margin, so THIS shape reaches
        // the restatement fold.)
        let handed_back = &fixtures[8];
        assert_eq!(
            handed_back.name,
            "research — a short question it must not simply hand back"
        );
        assert_eq!(
            handed_back.check("Is Redis Streams a good fit").as_deref(),
            Some("restates the input verbatim instead of naming it")
        );
        // The meta subject with every filler available to steal.
        let meta = &fixtures[9];
        assert_eq!(
            meta.name,
            "chat — a meta subject, with every filler word available to steal"
        );
        assert_eq!(
            meta.check("Discussion about how we run meetings")
                .as_deref(),
            Some("opens with a generic filler (\"Chat about\", \"Discussion of\")")
        );
    }

    #[test]
    fn there_are_ten_fixtures_across_three_bands() {
        let fixtures = fixtures();
        assert_eq!(fixtures.len(), 10);
        assert_eq!(
            fixtures.iter().filter(|f| f.band == EvalBand::Easy).count(),
            3
        );
        assert_eq!(
            fixtures
                .iter()
                .filter(|f| f.band == EvalBand::Standard)
                .count(),
            4
        );
        assert_eq!(
            fixtures.iter().filter(|f| f.band == EvalBand::Hard).count(),
            3
        );
    }

    // ── The def, driven through the runner against a recorded world ──────────

    async fn run(
        def: &HarnessDefinition,
        input: &Value,
        r: &Recorder,
    ) -> Result<HarnessResult, crate::harness::run::HarnessError> {
        let ctx = RunContext {
            caller: "test:titler".into(),
            deps: Some(r.deps()),
            ..Default::default()
        };
        execute(&r.deps(), def, input, ctx, None).await
    }

    fn chat_input(text: &str) -> Value {
        serde_json::json!({ "kind": "chat", "text": text })
    }

    fn research_input(text: &str) -> Value {
        serde_json::json!({ "kind": "research", "text": text })
    }

    #[tokio::test]
    async fn a_working_run_narrows_the_reply_into_a_title() {
        let def = titler_harness();
        let r = recorded_run(World {
            replies: replies(&["Checkout latency on mobile."]),
            ..Default::default()
        });
        let res = run(
            &def,
            &chat_input("user: our checkout page is slow on mobile"),
            &r,
        )
        .await
        .unwrap();
        assert_eq!(
            res.value,
            Some(Value::String("Checkout latency on mobile".into()))
        );
        assert!(res.answered && res.schema_valid);
        assert_eq!(res.model.as_deref(), Some("pl-main"));
        // The def's own facts, visible on the request the runner sent.
        let req = r.req_at(0);
        assert_eq!(req.temperature, Some(0.3));
        assert!(!req.json_mode);
        assert!(
            req.messages[0]
                .content
                .starts_with("Name this conversation. 3–7 words")
        );
        assert!(
            req.messages[0]
                .content
                .contains("Reply with ONLY the title.")
        );
        assert_eq!(
            req.messages[1].content,
            "user: our checkout page is slow on mobile"
        );
        // A text harness never gets the derived JSON floor, and its own floor
        // never blocks.
        assert!(def.floor.capabilities.is_empty() && !def.floor.refuse_below);
        assert_eq!(def.requires, ["instruction-following"]);
    }

    #[tokio::test]
    async fn the_prompt_names_the_kind() {
        let def = titler_harness();
        let r = recorded_run(World::default());
        run(
            &def,
            &serde_json::json!({ "kind": "plan", "text": "we need SSO working" }),
            &r,
        )
        .await
        .unwrap();
        assert!(
            r.req_at(0).messages[0]
                .content
                .starts_with("Name this plan.")
        );
        let r = recorded_run(World::default());
        run(
            &def,
            &research_input("How do European data residency rules apply?"),
            &r,
        )
        .await
        .unwrap();
        let system = r.req_at(0).messages[0].content.clone();
        assert!(system.starts_with("Name this research run from its question."));
        // The mismatch fix: every kind is told the rules, not just chat.
        assert!(system.contains("never a generic filler opening"));
    }

    #[tokio::test]
    async fn a_long_transcript_is_clipped_to_four_thousand_utf16_units() {
        let def = titler_harness();
        let r = recorded_run(World::default());
        // 6000 ASCII characters — 6000 UTF-16 units.
        let long = "x".repeat(6000);
        run(&def, &chat_input(&long), &r).await.unwrap();
        let sent = r.req_at(0).messages[1].content.clone();
        assert_eq!(utf16_len(&sent), 4000);
    }

    #[tokio::test]
    async fn a_reply_with_nothing_usable_fails_soft_and_the_caller_keeps_its_title() {
        let def = titler_harness();
        let r = recorded_run(World {
            replies: replies(&["```\n```"]),
            ..Default::default()
        });
        let res = run(&def, &chat_input("user: hello"), &r).await.unwrap();
        assert!(res.value.is_none() && !res.schema_valid);
        // The transport DID return non-empty text, so `answered` is true —
        // this is precisely the "a model answered and the answer was unusable"
        // case the failure policy exists to scope, and `OnFailure::Null` is
        // what keeps it soft.
        assert!(res.answered);
        assert!(
            res.error
                .as_deref()
                .is_some_and(|e| e.contains("clean step"))
        );
        assert_eq!(r.n_runs(), 1);
        assert!(r.run_at(0).error.is_some());
    }

    #[tokio::test]
    async fn a_decode_failure_is_a_failed_contract_not_a_panic() {
        // `kind: "surfing"` does not deserialize — the erasure point define.rs
        // states. OnFailure::Null keeps even this soft: the caller keeps its
        // title, and the run row says what happened.
        let def = titler_harness();
        let r = recorded_run(World::default());
        let res = run(
            &def,
            &serde_json::json!({ "kind": "surfing", "text": "no" }),
            &r,
        )
        .await
        .unwrap();
        assert!(res.value.is_none() && !res.answered);
        assert!(
            res.error
                .as_deref()
                .is_some_and(|e| e.contains("rendered no messages"))
        );
        assert_eq!(r.n_runs(), 1);
    }

    #[tokio::test]
    async fn a_titler_that_never_routes_answers_nothing_and_meters_the_try() {
        let def = titler_harness();
        let r = recorded_run(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        let res = run(&def, &chat_input("user: hello"), &r).await.unwrap();
        assert!(!res.answered && res.value.is_none());
        assert_eq!(r.n_requests(), 0);
        assert_eq!(r.n_runs(), 1);
    }

    #[tokio::test]
    async fn the_floor_runs_on_a_model_measured_unable_to_follow_instructions() {
        // runs_anyway: even a positively-measured-missing capability cannot
        // block the titler — the whole point of the empty floor.
        let def = titler_harness();
        let r = recorded_run(World {
            facts: facts(&[("spark", "instruction-following", probe(false))]),
            ..Default::default()
        });
        let res = run(&def, &chat_input("user: hello there"), &r)
            .await
            .unwrap();
        assert!(res.answered);
        assert_eq!(r.n_requests(), 1);
    }
}

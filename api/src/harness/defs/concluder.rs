// The Concluder: a relay's last word. Port of harness/defs/concluder.ts.
//
// Unlike the Distiller, this one is USER-INITIATED — somebody clicked "conclude"
// and is watching a spinner — so its failures are visible and its caller
// (`concludeRelay`, crossing with the channels plane) throws user-facing copy
// rather than swallowing a null. That difference is the whole reason the two
// harnesses in this pair declare different failure handling despite doing
// nearly the same job.
//
// PORTED FROM comms-decay.ts (audit 1.10). Prompt, temperature and user turn
// are the originals. The hand-copied model chain and the `if (!text.trim())`
// check are gone; the runner owns both.

use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::body::utf16_len;
use crate::harness::define::{
    CheckCtx, CheckResult, EvalBand, EvalCase, GuardDecl, HarnessDefinition, Message, OnFailure,
    Output, RenderContext, RoleFloor, Widen, define_harness,
};
use crate::harness_model::{MUSE_CHAIN, ModelSpec};

// camelCase on the wire — the TS def's declared JSON contract (`channelName`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcludeInput {
    /// The relay's name, as the participants know it.
    pub channel_name: String,
    /// The rendered transcript, already filtered and clipped by the caller.
    pub transcript: String,
}

// ── The prompts ──────────────────────────────────────────────────────────────

/// THE NARROW PROMPT, AND THE FLOOR IT WAS MISSING.
///
/// It used to be the first sentence alone. Two fixtures graded rules that only
/// the WIDENED prompt stated, and the sweep runs NARROW on every model that is
/// not widened — so most candidates were being graded on instructions they were
/// never given:
///
///   `leaves an unowned follow-up unowned instead of guessing at a name` — the
///   "leave it unattributed rather than guessing" clause lives in WIDE.
///   `does not turn a config change into an invented deliverable` — no prompt
///   said not to invent one. The distiller has "Never invent anything"; this
///   harness, which summarises a transcript the same way, had nothing.
///
/// WHAT WIDENING STILL BUYS is unchanged, and that matters: the fold below is
/// NEGATIVE (do not invent, do not guess an owner) while WIDE's ask is POSITIVE
/// (attribute every decision and every follow-up by name). A model that only
/// has the narrow half writes a correct unattributed summary; a widened one
/// writes an attributed one. Moving the negative half down does not make the
/// positive half redundant.
const NARROW: &str = "Write the closing summary for a work discussion: what was decided, what was produced, and any follow-ups — crisp markdown, a few bullets per section, no preamble.\nOnly what the transcript actually says: never invent a deliverable, an outcome or a date, and where it does not name who owns a follow-up, leave it unowned rather than guessing.";

/// The widened prompt. The narrow one already asks for sections, so widening
/// does not buy structure here — it buys ATTRIBUTION, which is the thing a
/// multi-party transcript has and a single-thread chat does not. Who decided
/// it, and who owns the follow-up, is what makes a concluded relay actionable
/// a week later.
///
/// It is gated rather than default because attribution is where a weak model
/// invents: asked for an owner on every follow-up, it assigns one, and a
/// summary that names the wrong person as accountable is worse than a summary
/// that names nobody. The "leave it unattributed" clause is the instruction
/// that has to hold, and that is what the capability gate is checking for.
const WIDE: &str = "Write the closing summary for a work discussion, in three sections: what was decided, what was produced,\nand any follow-ups. Crisp markdown, a few bullets per section, no preamble.\n\nAttribute each decision to whoever made it and each follow-up to whoever took it on, by the name they\nare given in the transcript. Where the transcript does not say, leave it unattributed rather than\nguessing - an unowned follow-up is useful and a misattributed one is worse than none.";

// ── The transcripts ──────────────────────────────────────────────────────────

/// A short relay with one decision, one deliverable and one follow-up — the
/// three sections the prompt asks for, so a summary that omits a section has
/// demonstrably omitted content rather than merely had nothing to say.
pub const FIXTURE: &str = "Priya: We need the export endpoint before the pilot.\n\nNomad: I have the CSV writer working. Streaming, so it holds on the big accounts.\n\nPriya: Good. Decision: we ship CSV only for the pilot, no XLSX.\n\nNomad: Understood. I pushed the endpoint and the fixture set this morning.\n\nPriya: Follow-up - somebody has to write the customer-facing note before Thursday.\n\nNomad: I can draft it.";

/// ONE DECISION, ONE DELIVERABLE, ONE FOLLOW-UP, and nothing to disentangle.
/// The easy floor: a model that cannot section this cannot section anything.
const SIMPLE: &str = "Ada: The staging certs expire Sunday.\n\nNomad: Renewed them this morning and pushed the new bundle.\n\nAda: Decision: we move to auto-renew so this stops happening.\n\nNomad: Follow-up - I will wire the ACME client this week.";

/// A relay where the DECISION IS A REVERSAL. The summary has to record what was
/// landed on, not both halves of the argument.
const REVERSED: &str = "Priya: I think we put the export behind a feature flag.\n\nNomad: that would let us dark-launch it.\n\nPriya: on reflection no - the flag is more moving parts than the feature. Decision: ship it unflagged to the pilot accounts only.\n\nNomad: unflagged, pilot accounts. Pushed the allowlist.\n\nPriya: Follow-up - review the allowlist with Support on Monday.";

/// A relay with UNATTRIBUTED follow-ups: nobody says who is doing it. The
/// widened prompt's hardest rule is "leave it unattributed rather than
/// guessing", and this is where a weak model invents an owner.
const UNOWNED: &str = "Marta: The importer times out on files over 200MB.\n\nNomad: confirmed, it is the synchronous parse.\n\nMarta: Decision: we chunk the parse rather than raising the timeout.\n\nNomad: I have the chunker prototyped.\n\nMarta: Follow-up - someone needs to benchmark it against the worst customer file before we ship.";

/// A relay that produced NO deliverable — a discussion that decided something
/// and built nothing. Inventing a deliverable to fill the section is the
/// failure.
const NO_DELIVERABLE: &str = "Ivan: Are we keeping the nightly rebuild?\n\nNomad: it costs about forty minutes of runner time a day.\n\nIvan: Decision: drop it, move to on-demand rebuilds from the release branch.\n\nNomad: nothing to build for that, it is a config change on the schedule.\n\nIvan: Follow-up - Ivan will delete the schedule after the current train ships.";

/// A relay whose decision is buried mid-thread, under a cost exchange nobody
/// needs to see again.
const BURIED: &str = "Sam: the invoice PDF renders wrong on Windows.\n\nNomad: it is the font fallback — the embedded subset is missing on their reader.\n\nSam: how bad?\n\nNomad: cosmetic, but it looks unprofessional on the total line.\n\nSam: fine. Decision: we embed the full font rather than the subset, and eat the extra 400KB per invoice.\n\nNomad: done, pushed the change and regenerated last month\u{2019}s invoices.\n\nSam: anything else outstanding?\n\nNomad: no.\n\nSam: Follow-up - tell Support the old PDFs are being regenerated so they stop getting tickets about it.";

// ── Eval assertions ──────────────────────────────────────────────────────────

/// Bullet or heading lines. A "few bullets per section" summary that comes back
/// as one prose paragraph has ignored the only formatting instruction it was
/// given, which is the reliable small-model tell on this harness.
fn bullet_line() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*(?:[-*+]\s+|#{1,6}\s+|\d+[.)]\s+)").unwrap())
}

fn structured_lines(value: &str) -> usize {
    value.lines().filter(|l| bullet_line().is_match(l)).count()
}

/// THE FORMATTING HALF, with a floor under it. `structured_lines` alone is an
/// upper-bound-free NOT: three bullets saying nothing satisfies it. The length
/// floor is what makes it an assertion about a summary rather than about
/// punctuation.
pub fn section_problem(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Some("the summary was empty".into());
    }
    let units = utf16_len(trimmed);
    if units < 60 {
        return Some(format!(
            "the summary is {units} characters — too short to carry three sections"
        ));
    }
    let lines = structured_lines(value);
    if lines >= 3 {
        None
    } else {
        Some(format!(
            "expected at least 3 bullet or heading lines, got {lines} - the summary came back as prose"
        ))
    }
}

/// THE CONTENT HALF: the load-bearing tokens of this relay, each with the
/// human sentence naming what its absence means.
pub fn carries(value: &str, wanted: &[(&str, &str)]) -> Option<String> {
    let v = value.to_lowercase();
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|(token, _)| !v.contains(token))
        .map(|(_, label)| *label)
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!("left out {}", missing.join(", ")))
    }
}

// ── The fixture-specific tails ───────────────────────────────────────────────

/// The relay explicitly says there is nothing to build. A summary that reports
/// shipped code has invented the one thing nobody did.
fn invented_a_deliverable(value: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let invented = R
        .get_or_init(|| {
            Regex::new(r"(?i)\b(?:shipped|merged|deployed|released|built the)\b").unwrap()
        })
        .is_match(value);
    if invented {
        Some(
            "reported a deliverable on a relay whose participants said there was nothing to build"
                .into(),
        )
    } else {
        None
    }
}

/// Naming the flag as the rejected option is fine; presenting it as the
/// decision is the failure.
fn recorded_the_reversed_decision(value: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let reversed = R
        .get_or_init(|| {
            Regex::new(r"(?i)decision[^\n]*feature flag|ship[^\n]*behind (?:a )?(?:feature )?flag")
                .unwrap()
        })
        .is_match(value);
    if reversed {
        Some("recorded the reversed decision (the feature flag) as if it still stood".into())
    } else {
        None
    }
}

/// The widened prompt's hardest clause. Nobody in the relay takes the
/// benchmark, and a model asked for an owner on every follow-up will assign
/// one — which is worse than naming nobody.
fn guessed_an_unowned_follow_up(value: &str) -> Option<String> {
    let follow_up: String = value
        .lines()
        .filter(|l| l.to_lowercase().contains("benchmark"))
        .collect::<Vec<_>>()
        .join(" ");
    if follow_up.is_empty() {
        return Some("left out the follow-up entirely".into());
    }
    // Marta and Nomad are the only names in the transcript, and neither
    // volunteered for it.
    static R: OnceLock<Regex> = OnceLock::new();
    let invented = R
        .get_or_init(|| Regex::new(r"\b(?:Marta|Nomad)\b").unwrap())
        .find(&follow_up);
    invented.map(|m| {
        format!(
            "attributed the unowned follow-up to {}, who never took it on",
            m.as_str()
        )
    })
}

/// A summary at least as long as the relay it closes restated it rather than
/// concluding it.
fn no_shorter_than_the_relay(value: &str) -> Option<String> {
    if utf16_len(value) < utf16_len(FIXTURE) {
        None
    } else {
        Some("the summary is no shorter than the relay it closes — the model restated it rather than concluding it".into())
    }
}

// ── The fixtures ─────────────────────────────────────────────────────────────

/// One fixture: the section fold, then the load-bearing tokens, then the
/// bespoke tail the fixture exists to make. `sections: false` spells the TS
/// fixtures that chained straight to `carries` (or to their own fold) without
/// the formatting half.
pub struct ConcludeFixture {
    pub name: &'static str,
    pub band: EvalBand,
    pub input: ConcludeInput,
    pub sections: bool,
    pub carries: &'static [(&'static str, &'static str)],
    pub extra: Option<fn(&str) -> Option<String>>,
}

impl ConcludeFixture {
    pub fn check(&self, value: &str) -> Option<String> {
        if self.sections
            && let Some(problem) = section_problem(value)
        {
            return Some(problem);
        }
        if !self.carries.is_empty()
            && let Some(problem) = carries(value, self.carries)
        {
            return Some(problem);
        }
        self.extra.and_then(|extra| extra(value))
    }
}

/// NINE FIXTURES, THREE BANDS. Both originals ran on the same transcript, so a
/// model that happened to section THAT relay scored a perfect concluder. The
/// shapes below are the ones that break the job differently: a reversal, a
/// relay with nobody named, a relay that produced nothing, and one long enough
/// that keeping every section takes actually reading it.
pub fn fixtures() -> Vec<ConcludeFixture> {
    vec![
        ConcludeFixture {
            name: "sections a short, unambiguous relay",
            band: EvalBand::Easy,
            input: ConcludeInput {
                channel_name: "certs".into(),
                transcript: SIMPLE.into(),
            },
            sections: true,
            carries: &[
                ("cert", "the expiring certs"),
                ("renew", "the renewal decision"),
            ],
            extra: None,
        },
        ConcludeFixture {
            name: "comes back as sections rather than a paragraph",
            band: EvalBand::Easy,
            input: ConcludeInput {
                channel_name: "pilot-export".into(),
                transcript: FIXTURE.into(),
            },
            sections: true,
            carries: &[],
            extra: None,
        },
        ConcludeFixture {
            name: "carries the decision, the deliverable and the follow-up",
            band: EvalBand::Standard,
            input: ConcludeInput {
                channel_name: "pilot-export".into(),
                transcript: FIXTURE.into(),
            },
            sections: false,
            carries: &[
                ("csv", "the decision to ship CSV only"),
                ("endpoint", "the endpoint that was produced"),
                ("thursday", "the follow-up and its deadline"),
            ],
            extra: None,
        },
        ConcludeFixture {
            name: "keeps all three sections on a relay with plenty to lose",
            band: EvalBand::Standard,
            input: ConcludeInput {
                channel_name: "importer".into(),
                transcript: UNOWNED.into(),
            },
            sections: true,
            carries: &[
                ("chunk", "the decision to chunk the parse"),
                ("benchmark", "the follow-up"),
            ],
            extra: None,
        },
        ConcludeFixture {
            name: "does not turn a config change into an invented deliverable",
            band: EvalBand::Standard,
            input: ConcludeInput {
                channel_name: "ci".into(),
                transcript: NO_DELIVERABLE.into(),
            },
            sections: true,
            carries: &[("on-demand", "the decision"), ("schedule", "the follow-up")],
            extra: Some(invented_a_deliverable),
        },
        ConcludeFixture {
            name: "names the channel\u{2019}s subject rather than restating the transcript",
            band: EvalBand::Standard,
            input: ConcludeInput {
                channel_name: "pilot-export".into(),
                transcript: FIXTURE.into(),
            },
            // THE SECTION FOLD FIRST. On its own the tail is a pure upper bound,
            // and a fourteen-character non-answer is shorter than any
            // transcript — the one-sided assertion the sweep's own garbage
            // census exists to catch, and it caught this one in draft.
            sections: true,
            carries: &[("csv", "the decision")],
            extra: Some(no_shorter_than_the_relay),
        },
        // ── hard ──────────────────────────────────────────────────────────────
        ConcludeFixture {
            name: "records the position the relay landed on, not the one it abandoned",
            band: EvalBand::Hard,
            input: ConcludeInput {
                channel_name: "pilot-export".into(),
                transcript: REVERSED.into(),
            },
            sections: true,
            carries: &[
                ("unflagged", "the decision that was actually taken"),
                ("allowlist", "the deliverable"),
            ],
            extra: Some(recorded_the_reversed_decision),
        },
        ConcludeFixture {
            name: "leaves an unowned follow-up unowned instead of guessing at a name",
            band: EvalBand::Hard,
            input: ConcludeInput {
                channel_name: "importer".into(),
                transcript: UNOWNED.into(),
            },
            sections: false,
            carries: &[],
            extra: Some(guessed_an_unowned_follow_up),
        },
        ConcludeFixture {
            name: "sections a relay whose decision is buried mid-thread",
            band: EvalBand::Hard,
            input: ConcludeInput {
                channel_name: "billing".into(),
                transcript: BURIED.into(),
            },
            sections: true,
            carries: &[
                ("font", "the decision about the embedded font"),
                ("support", "the follow-up to Support"),
            ],
            extra: None,
        },
    ]
}

// ── The def ──────────────────────────────────────────────────────────────────

pub fn concluder_harness() -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        "concluder",
        "Concluder",
        "Writes the closing summary when a relay concludes — decisions, deliverables, follow-ups.",
        // The chain comms-decay hand-wrote: the Concluder's assigned model,
        // else the concluding user's own muse. That order IS the muse chain,
        // and this is its second spelt use — one spelling, referenced rather
        // copied, so an admin who reorders the muse's resolution reorders
        // every "the user's own assistant" fallback with it. `user_id` comes
        // from the RUN CONTEXT, not the def: the runner threads ctx's user
        // into the resolve edge, which turns on the 'preferred' step and the
        // member model allowlist for whoever clicked conclude.
        ModelSpec {
            pin: Some("concluder"),
            role: None,
            chain: Some(&MUSE_CHAIN),
            user_id: None,
        },
        Arc::new(|input: &Value, ctx: &RenderContext| {
            let conclude: ConcludeInput =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            // The FIRST render branch on `widened` in the ported defs — the
            // capability-gated attribution ask (see WIDE above).
            let system = if ctx.widened { WIDE } else { NARROW };
            Ok(vec![
                Message::system(system),
                Message::user(format!(
                    "Relay \"{}\":\n\n{}",
                    conclude.channel_name, conclude.transcript
                )),
            ])
        }),
        Output::Text {
            clean: Some(Arc::new(|raw: &str| {
                let t = raw.trim();
                if t.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Value::String(t.to_string())))
                }
            })),
            verify: None,
        },
        // NOT Throw. `run_harness` returns rather than throws when no model
        // resolves at all, so Throw would cover one of this caller's two
        // failure modes and silently skip the other. `concludeRelay`
        // distinguishes them itself — a null MODEL means "nothing is
        // configured to summarize with", a null VALUE means "it was asked and
        // answered with nothing" — and those are different sentences for the
        // person watching the spinner.
        OnFailure::Null,
    ));
    // Same reasoning as the distiller: the transcript is clipped, so the ask is
    // bounded, and what the job actually leans on is holding several
    // formatting and content constraints in one system turn.
    d.requires = vec!["instruction-following"];
    d.floor = RoleFloor::runs_anyway(
        // Refusing would be the wrong trade even though this one is
        // user-facing: a loose summary is still a record, and the person is
        // right there to read it and re-run. Failure is already visible here —
        // `concludeRelay` throws and the UI shows the message — so there is
        // nothing for a floor to protect.
        "A smaller model writes a looser summary; if it returns nothing at all the relay stays open and you can conclude it again.",
    );
    d.widen = Some(Widen {
        requires: vec!["instruction-following"],
        note: "Models proven to follow a \"leave it unattributed rather than guess\" instruction also name who decided what and who owns each follow-up.",
    });
    // Narrowed for the same reason as the distiller: a closing summary REPORTS
    // what agents did and what broke, so `zero_tool_claim` and
    // `fabricated_outage` fire on it doing its job. `redact` is on because
    // this text is posted back into the channel and indexed for retrieval, so
    // a credential quoted out of the transcript would outlive the relay in two
    // places at once.
    d.guard = Some(GuardDecl {
        rules: Some(vec!["secret_leak", "pii_leak"]),
        redact: true,
    });
    d.temperature = Some(0.2);
    // THE FIXTURE TABLE, folded onto the fitness plane's `EvalCase`. Each row
    // composes its own check at fold time — the formatting half, the
    // load-bearing tokens, and the bespoke tail the fixture exists to make —
    // so the fold itself only has to re-type the reply (the def's `Output` is
    // `Text`, and its clean step already trimmed to a bare JSON string) and
    // hand it to `ConcludeFixture::check`. A value that does not decode is the
    // sweep's thrown check, written as a FAIL sentence.
    //
    // No `dry_run`: a concluder turn calls no tools — the transcript arrives
    // rendered and clipped by `concludeRelay`, and the reply is a single
    // string — so a replay of these rows runs single-shot against the empty
    // context and needs no world.
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
                        Err(e) => CheckResult::Fail(format!(
                            "the fixture check threw on the value: {e}"
                        )),
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
        RecordedRun as Recorder, RecordedWorld as World, facts, probe, recorded_run, replies,
    };
    use crate::harness::run::{HarnessResult, RunContext, execute};

    // ── The folds ────────────────────────────────────────────────────────────

    #[test]
    fn section_problem_needs_both_halves() {
        assert_eq!(
            section_problem("   ").as_deref(),
            Some("the summary was empty")
        );
        assert_eq!(
            section_problem("- short").as_deref(),
            Some("the summary is 7 characters — too short to carry three sections")
        );
        // Three bullets saying nothing pass the formatting half — which is why
        // the fixtures pair it with `carries`, and why the content half exists.
        let bullets = "- one two three four five six seven\n- eight nine ten eleven twelve\n- thirteen fourteen fifteen";
        assert!(section_problem(bullets).is_none());
        // A long prose paragraph, however good, ignored the only formatting
        // instruction the prompt gave.
        let prose = "w".repeat(80);
        assert_eq!(
            section_problem(&prose).as_deref(),
            Some(
                "expected at least 3 bullet or heading lines, got 0 - the summary came back as prose"
            )
        );
    }

    #[test]
    fn structured_lines_counts_each_marker_shape() {
        assert_eq!(
            structured_lines("- a\n* b\n+ c\n## d\n1. e\n2) f\nplain\ngone"),
            6
        );
    }

    #[test]
    fn carries_names_what_is_missing_in_order() {
        assert_eq!(
            carries(
                "mentions csv and the thursday deadline, so nothing is missing",
                &[("csv", "the decision"), ("thursday", "the deadline")]
            )
            .as_deref(),
            None
        );
        assert_eq!(
            carries(
                "mentions nothing",
                &[("csv", "the decision"), ("thursday", "the deadline")]
            )
            .as_deref(),
            Some("left out the decision, the deadline")
        );
    }

    // ── The fixtures ─────────────────────────────────────────────────────────

    #[test]
    fn every_fixture_passes_a_sectioned_summary_that_carries_the_load() {
        let good = [
            // certs/SIMPLE
            "## Decided\n- Staging certs move to auto-renew so the expiry stops\n\n## Produced\n- Renewed bundle pushed\n\n## Follow-ups\n- Wire the ACME client this week",
            // sections only
            "## Decided\n- Ship CSV only\n\n## Produced\n- Endpoint and fixtures\n\n## Follow-ups\n- Customer note",
            // carries only (no section requirement)
            "Decided: CSV only for the pilot, no XLSX. Produced: the streaming endpoint. Follow-up: the customer-facing note before Thursday.",
            // importer/UNOWNED
            "## Decided\n- Chunk the parse rather than raising the timeout\n\n## Produced\n- Chunker prototype\n\n## Follow-ups\n- Someone has to benchmark it against the worst customer file before shipping",
            // ci/NO_DELIVERABLE
            "## Decided\n- Drop the nightly rebuild for on-demand rebuilds from the release branch\n\n## Follow-ups\n- Ivan deletes the schedule after the current train",
            // no-shorter
            "## Decided\n- CSV only for the pilot\n\n## Produced\n- Export endpoint\n\n## Follow-ups\n- Customer note by Thursday",
            // REVERSED
            "## Decided\n- Ship unflagged to the pilot accounts only; the flag idea was dropped as more moving parts\n\n## Produced\n- The allowlist is pushed\n\n## Follow-ups\n- Review the allowlist with Support on Monday",
            // UNOWNED bespoke
            "## Decided\n- Chunk the parse\n\n## Follow-ups\n- Someone has to benchmark the chunker against the worst customer file",
            // billing/BURIED
            "## Decided\n- Embed the full font rather than the subset, eating the extra 400KB per invoice\n\n## Produced\n- Change pushed, last month's invoices regenerated\n\n## Follow-ups\n- Tell Support the old PDFs are being regenerated",
        ];
        for (fixture, value) in fixtures().iter().zip(good) {
            assert!(
                fixture.check(value).is_none(),
                "{}: {:?} -> {:?}",
                fixture.name,
                value,
                fixture.check(value)
            );
        }
    }

    #[test]
    fn the_named_traps_fire_their_own_sentences() {
        let fixtures = fixtures();
        // A config change dressed up as shipped code.
        let invented = &fixtures[4];
        assert_eq!(
            invented.name,
            "does not turn a config change into an invented deliverable"
        );
        assert_eq!(
            invented
                .check("## Decided\n- On-demand rebuilds from the release branch\n\n## Produced\n- The schedule change was deployed\n\n## Follow-ups\n- Delete the schedule")
                .as_deref(),
            Some("reported a deliverable on a relay whose participants said there was nothing to build")
        );
        // The reversal recorded as if it stood — and note the value still has
        // to CARRY the real decision ("unflagged") to reach the tail, which is
        // the point: a summary that omits the outcome fires `carries` first.
        let reversed = &fixtures[6];
        assert_eq!(
            reversed.name,
            "records the position the relay landed on, not the one it abandoned"
        );
        assert_eq!(
            reversed
                .check("## Decided\n- Decision: ship it behind a feature flag; the unflagged option was dropped as more moving parts\n\n## Produced\n- The allowlist is pushed\n\n## Follow-ups\n- Review with Support on Monday")
                .as_deref(),
            Some("recorded the reversed decision (the feature flag) as if it still stood")
        );
        // The unowned follow-up, guessed at — and omitted outright.
        let unowned = &fixtures[7];
        assert_eq!(
            unowned.name,
            "leaves an unowned follow-up unowned instead of guessing at a name"
        );
        assert_eq!(
            unowned
                .check("## Decided\n- Chunk the parse\n\n## Follow-ups\n- Marta will benchmark it")
                .as_deref(),
            Some("attributed the unowned follow-up to Marta, who never took it on")
        );
        assert_eq!(
            unowned
                .check("## Decided\n- Chunk the parse\n\n## Produced\n- Prototype\n\n## Notes\n- Nothing else")
                .as_deref(),
            Some("left out the follow-up entirely")
        );
        // The restatement: a summary as long as the relay it closes. It has to
        // pass the section fold first — three real bullets, all the
        // load-bearing tokens — and STILL be as long as the transcript.
        let restates = &fixtures[5];
        assert_eq!(
            restates.name,
            "names the channel\u{2019}s subject rather than restating the transcript"
        );
        let long = "## What was decided\n- Priya decided that we ship the CSV export only for the pilot, with no XLSX in this iteration, because the endpoint has to land before the pilot accounts are onboarded and a second format would double the surface she has to review\n\n## What was produced\n- Nomad pushed the export endpoint along with its fixture set, and confirmed the writer streams so it holds up on the big accounts rather than buffering the whole file in memory the way the first draft did\n\n## Follow-ups\n- Somebody still has to write the customer-facing note before Thursday, and Nomad offered to draft it once the endpoint review is done";
        assert_eq!(
            restates.check(long).as_deref(),
            Some(
                "the summary is no shorter than the relay it closes — the model restated it rather than concluding it"
            )
        );
    }

    #[test]
    fn nine_fixtures_across_three_bands() {
        let fixtures = fixtures();
        assert_eq!(fixtures.len(), 9);
        assert_eq!(
            fixtures.iter().filter(|f| f.band == EvalBand::Easy).count(),
            2
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
            caller: "test:concluder".into(),
            deps: Some(r.deps()),
            ..Default::default()
        };
        execute(&r.deps(), def, input, ctx, None).await
    }

    fn conclude_input(channel: &str, transcript: &str) -> Value {
        serde_json::json!({ "channelName": channel, "transcript": transcript })
    }

    #[tokio::test]
    async fn a_narrow_run_renders_the_narrow_prompt_and_trims_the_reply() {
        let def = concluder_harness();
        let reply = "  ## Decided\n- Ship CSV only for the pilot\n\n## Produced\n- Export endpoint\n\n## Follow-ups\n- Customer note by Thursday  ";
        let r = recorded_run(World {
            replies: replies(&[reply]),
            ..Default::default()
        });
        let res = run(&def, &conclude_input("pilot-export", FIXTURE), &r)
            .await
            .unwrap();
        let value = res
            .value
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(value.starts_with("## Decided"));
        assert!(value.ends_with("Thursday"));
        assert!(res.answered && res.schema_valid);
        // The def's own facts, visible on the request the runner sent.
        let req = r.req_at(0);
        assert_eq!(req.temperature, Some(0.2));
        assert!(
            req.messages[0]
                .content
                .starts_with("Write the closing summary for a work discussion: what was decided")
        );
        assert!(
            req.messages[0]
                .content
                .contains("leave it unowned rather than guessing")
        );
        assert_eq!(
            req.messages[1].content,
            format!("Relay \"pilot-export\":\n\n{FIXTURE}")
        );
    }

    #[tokio::test]
    async fn a_model_proven_to_follow_instructions_gets_the_attribution_ask() {
        // THE FIRST WIDEN BRANCH IN THE PORTED DEFS. Widening demands evidence
        // — a probe, not a claim — and what it buys here is attribution.
        let def = concluder_harness();
        let r = recorded_run(World {
            facts: facts(&[("spark", "instruction-following", probe(true))]),
            ..Default::default()
        });
        let res = run(&def, &conclude_input("certs", FIXTURE), &r)
            .await
            .unwrap();
        assert!(res.widened);
        let system = r.req_at(0).messages[0].content.clone();
        assert!(
            system
                .starts_with("Write the closing summary for a work discussion, in three sections")
        );
        assert!(system.contains("Attribute each decision to whoever made it"));
        // Unproven, the same run stays NARROW — unknown does not widen.
        let r = recorded_run(World::default());
        let res = run(&def, &conclude_input("certs", FIXTURE), &r)
            .await
            .unwrap();
        assert!(!res.widened);
        assert!(
            r.req_at(0).messages[0]
                .content
                .contains("Only what the transcript actually says")
        );
    }

    #[tokio::test]
    async fn a_blank_reply_keeps_the_relay_open() {
        let def = concluder_harness();
        let r = recorded_run(World {
            replies: replies(&["   \n  "]),
            ..Default::default()
        });
        let res = run(&def, &conclude_input("certs", FIXTURE), &r)
            .await
            .unwrap();
        assert!(res.value.is_none() && !res.schema_valid);
        // Whitespace is not an answer — `answered` is the transport's flag,
        // and this transport returned nothing but spaces.
        assert!(!res.answered);
        assert!(
            res.error
                .as_deref()
                .is_some_and(|e| e.contains("clean step"))
        );
    }
}

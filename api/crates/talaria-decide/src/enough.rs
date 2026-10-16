// IS THAT ENOUGH? — the one site that spends nothing and saves whole rounds.
//
// A research run is given a budget in rounds before it starts: one for a
// recon, two for a brief, four for an expedition. It then spends every one of
// them, because nothing in the loop has ever been able to answer the only
// question that would stop it early — do the findings so far actually answer
// what was asked? A model writing the report at round four has read the same
// notes the judge would read at round two, and on a question that turned out
// to be easy it was paid to re-read them twice.
//
// WHY THIS ONE ACTS, when the scoper's verdict next door does not. There is no
// prose coupled to this answer. A yes means "go to Synthesize", which is the
// stage the run was going to reach anyway, with the findings it already has
// and the report it was always going to write. Nothing is discarded, nobody is
// un-asked a question, and the document a reader ends up with is the same
// document — written sooner, off the same notes. That is the difference
// between a judgment that can be acted on today and one that needs a
// restructure first, and it is why this site ships on.
//
// WHEN IT IS NEVER ASKED, which is as much of the design as the question is.
// The driver asks exactly at a round boundary with rounds still remaining:
//
//   NEVER ON THE LAST ROUND. There is nothing left to save — the next stage is
//   Synthesize either way — so the question would be a paid call to confirm
//   what the budget already decided.
//
//   NEVER WITH NO SOURCES YET. "Do these findings answer the ask" over an
//   empty list is not a judgment, it is a coin flip with a prompt on it. A
//   round that found nothing citable is the run's own retry path, not this
//   one's.
//
// THE BASELINE IS "CONTINUE", because that is what the existing code chose at
// every one of these boundaries, and the comparison is the whole ledger: the
// port AGREES when it says no. A high agreement rate here means "it would
// rarely have stopped early" rather than "it is right", which is the reading
// `channel-speech`'s rows need for the same reason.

use serde_json::{Value, json};
use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Answer, Judgment, Question, ask_one, shadow, sites};

/// The census id this site records and reads its switch under.
pub const SITE: &str = "research-enough";

/// HOW MANY FINDINGS THE JUDGMENT READS. An expedition can run twenty
/// queries, and a prompt carrying all of them would be most of the report.
pub const MAX_FINDINGS: usize = 12;

/// How much of each one. A finding is a cited markdown note; the first
/// paragraphs carry what it found and the tail carries its sources.
pub const FINDING_CHARS: usize = 1200;

/// What the existing loop chose at every one of these boundaries.
const BASELINE: &str = "continue";

/// THE PORT AGREED WITH THE BUDGET WHEN IT SAID NO. `noul_agrees` reads a
/// baseline spelled "true"/"false" and this one is a word, so the comparison
/// is written out: the budget's answer is always "keep going", so a judgment
/// below the middle is the one that matches it.
fn agrees_with_the_budget(answer: &Answer, _baseline: &str) -> bool {
    answer.probability().is_some_and(|p| p < 0.5)
}

/// One finding as the question sees it: trimmed and clamped.
fn clamped(finding: &str) -> String {
    talaria_body::truncate_utf16(finding.trim(), FINDING_CHARS).to_string()
}

/// The state the question is asked over.
///
/// THE TAIL IS WHAT IS KEPT when there are more than `MAX_FINDINGS`, and that
/// choice is load-bearing rather than arbitrary. This question is asked again
/// at every round boundary, so a judgment that cannot see what the latest
/// round added would answer it the same way it answered last time — the site
/// would either stop every run at its first boundary or never fire at all.
/// The findings that changed since the last no are the ones the answer turns
/// on.
///
/// `scopeRead` is present only when the scope stage produced one: a null in a
/// prompt is a field the model has to interpret, and "the ask, restated" with
/// nothing after it is worse than the ask alone.
fn state_of(
    question: &str,
    scope_read: Option<&str>,
    findings: &[String],
    round: u64,
    rounds: u64,
    mode: &str,
) -> Value {
    let keep = findings.len().saturating_sub(MAX_FINDINGS);
    let mut state = json!({
        "ask": question.trim(),
        "findings": findings[keep..].iter().map(|f| clamped(f)).collect::<Vec<_>>(),
        // ROUNDS USED, NOT THE ROUND NUMBER: the question is asked after round
        // N has come back, so N rounds of searching are what the findings
        // represent.
        "roundsUsed": round,
        "roundsTotal": rounds,
        // DEPTH-RELATIVE, exactly as crispness is next door: the same findings
        // are a finished recon and a third of an expedition, and a judgment
        // that cannot see which was asked for is answering a different
        // question than the run was given.
        "depth": mode,
    });
    if let Some(read) = scope_read.map(str::trim).filter(|r| !r.is_empty())
        && let Some(obj) = state.as_object_mut()
    {
        obj.insert("scopeRead".into(), json!(read));
    }
    state
}

/// Judge whether the findings so far answer the ask.
///
/// Answers the judgment rather than a boolean, so the caller can record it
/// before it decides whether it is allowed to act on it — an agreement rate
/// computed over only the acted-on half is not one.
///
/// `None` when there is nothing to judge: an empty ask, or no findings yet.
#[allow(clippy::too_many_arguments)] // each argument names one fact the question reads
pub async fn judge_enough(
    state: &AppState,
    http: &HttpFetch,
    question: &str,
    scope_read: Option<&str>,
    findings: &[String],
    round: u64,
    rounds: u64,
    mode: &str,
) -> Option<Judgment> {
    if question.trim().is_empty() || findings.iter().all(|f| f.trim().is_empty()) {
        return None;
    }
    ask_one(
        state,
        http,
        state_of(question, scope_read, findings, round, rounds, mode),
        Question::noul_meaning(
            "Do the findings below already answer the `ask` well enough to write the report at the stated `depth`, with no further searching?",
            "the findings cover what was asked, at the depth asked for, with enough agreeing detail to write from — another round of searching would add material nobody asked for",
            "something the ask needs is still missing, thin, or contradicted between findings, and another round of searching would change what the report says",
        ),
    )
    .await
}

/// Does an ACTED judgment mean "write the report now"?
///
/// ONE DIRECTION, read explicitly rather than taken from `acted` returning
/// `Some`. The site reads its floor as the probability of yes today, so the
/// two happen to agree — but the reading is a census field and the floor is an
/// operator's, and under `FLOOR_CERTAINTY` a CONFIDENT NO also clears. A "not
/// yet, definitely" that ended the run would be the exact inversion of this
/// question.
fn says_enough(judged: Option<Judgment>) -> bool {
    judged
        .and_then(|j| j.answer.probability())
        .is_some_and(|p| p >= 0.5)
}

/// THE RUN'S OWN DOOR: ask, record, and answer whether to stop early.
///
/// `false` is what the loop did before this site existed — with the port off,
/// the site off, no judgment, or a judgment below the floor, a run spends
/// every round it was given, exactly as it always has.
#[allow(clippy::too_many_arguments)] // the run's facts, one argument each — same list as judge_enough plus the row the ledger points at
pub async fn enough_now(
    state: &AppState,
    http: &HttpFetch,
    run_id: &str,
    question: &str,
    scope_read: Option<&str>,
    findings: &[String],
    round: u64,
    rounds: u64,
    mode: &str,
) -> bool {
    let judged = judge_enough(
        state, http, question, scope_read, findings, round, rounds, mode,
    )
    .await;
    shadow::record(
        &state.pg,
        &shadow::Compare {
            site: SITE,
            // The run, which a reader can open under the permissions they
            // already have — the ask itself is a person's sentence.
            subject_ref: Some(run_id.to_string()),
            baseline: BASELINE.to_string(),
            agrees: agrees_with_the_budget,
        },
        judged.as_ref(),
    )
    .await;
    says_enough(sites::acted(&state.pg, SITE, judged).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judgment(p: f64, calibrated: bool) -> Judgment {
        Judgment {
            answer: Answer::Noul(p),
            calibrated,
            provider: "test".into(),
            model: None,
            latency_ms: 1,
            tokens_in: None,
            tokens_out: None,
            fanned: 1,
        }
    }

    #[test]
    fn the_state_carries_the_tail_of_the_findings_and_not_all_of_them() {
        // Twenty queries is a real expedition, and the whole set would be most
        // of the report. The TAIL is kept because this question gets asked
        // again at every boundary — a judgment that cannot see the latest
        // round answers it the same way it answered last time.
        let findings: Vec<String> = (0..20).map(|i| format!("finding {i}")).collect();
        let state = state_of("does it?", None, &findings, 3, 4, "expedition");
        let kept = state["findings"].as_array().expect("an array");
        assert_eq!(kept.len(), MAX_FINDINGS);
        assert_eq!(kept[0], json!("finding 8"));
        assert_eq!(kept[MAX_FINDINGS - 1], json!("finding 19"));
        assert_eq!(state["roundsUsed"], json!(3));
        assert_eq!(state["roundsTotal"], json!(4));
        assert_eq!(state["depth"], json!("expedition"));
        // Absent, not null: a null here is a field the model has to interpret.
        assert!(state.get("scopeRead").is_none());
    }

    #[test]
    fn a_long_finding_is_clamped_and_the_cut_lands_on_a_character() {
        // A finding is a cited note; a run of them uncut would be the report.
        // The cut is by utf16 unit — the same count every body bound in this
        // repo uses — so a multi-byte character is never split in half.
        let long = "é".repeat(2_000);
        let state = state_of("does it?", Some("  "), &[long], 1, 2, "brief");
        let kept = state["findings"][0].as_str().expect("a string");
        assert_eq!(kept.chars().count(), FINDING_CHARS);
        assert!(kept.chars().all(|c| c == 'é'), "no split character");
        // A whitespace-only read is no read at all.
        assert!(state.get("scopeRead").is_none());
    }

    #[test]
    fn the_scope_read_rides_along_when_there_is_one() {
        let state = state_of(
            "does it?",
            Some("whether pg17 is faster"),
            &["a".into()],
            1,
            2,
            "brief",
        );
        assert_eq!(state["scopeRead"], json!("whether pg17 is faster"));
    }

    #[test]
    fn only_a_yes_ends_the_run_early_and_a_confident_no_never_does() {
        // THE ONE-DIRECTION RULE, pinned without a database: a confident "not
        // yet" means keep going, which is what the budget already does. Read
        // explicitly, because a census that moved this site to FLOOR_CERTAINTY
        // would otherwise hand `acted` a confident NO to stop the run on.
        assert!(says_enough(Some(judgment(0.9, true))));
        assert!(!says_enough(Some(judgment(0.05, true))));
        assert!(!says_enough(None));
    }

    #[test]
    fn the_ledger_counts_a_no_as_agreeing_with_the_budget() {
        // The baseline is a word, not "true"/"false", so `noul_agrees` cannot
        // read it — this is the comparison written out. A no agrees with the
        // loop that was going to keep going.
        assert!(agrees_with_the_budget(&Answer::Noul(0.1), BASELINE));
        assert!(!agrees_with_the_budget(&Answer::Noul(0.9), BASELINE));
    }

    #[test]
    fn this_site_is_in_the_census_and_ships_switched_on() {
        let def = sites::def_of(SITE).expect("research-enough is in the census");
        assert_eq!(def.primitive, "noul");
        assert_eq!(def.reads, sites::FLOOR_LEAN);
        // On by default, which is only defensible because the site cannot
        // spend anything — and the census sentence has to say what it ends.
        assert!(def.default_on);
        assert!(def.acts.contains("early"), "{}", def.acts);
        // And an uncalibrated answer still acts on nothing, on by default or
        // not: a fabricated confidence looks like evidence.
        let gate = sites::SiteGate {
            on: true,
            floor: def.default_floor,
        };
        assert!(says_enough(sites::acts_on(
            def,
            &gate,
            Some(judgment(0.9, true))
        )));
        assert!(!says_enough(sites::acts_on(
            def,
            &gate,
            Some(judgment(0.9, false))
        )));
    }
}

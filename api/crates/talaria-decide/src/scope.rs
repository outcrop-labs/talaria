// IS THE ASK CRISP ENOUGH TO RESEARCH — the research scoper's verdict half.
//
// The scoper runs before anything is planned or searched and answers
// `{crisp, read, questions}`: whether the ask can be worked as typed, one or
// two sentences saying it back, and — exactly when it is not crisp — the
// questions a person has to answer first. `crisp` is a boolean, so it is a
// Noul, and it arrives today as a JSON field a text model wrote with no number
// on it.
//
// WHY THE VERDICT IS NOT SIMPLY REPLACED, and it is the same structural reason
// as `action` next door — worth stating once about the class rather than twice
// about the instances. THE VERDICT IS COUPLED TO ITS PROSE. `questions` is
// non-empty exactly when `crisp` is false, and both halves are posted into the
// run's discussion for a person to read. So a typed answer cannot stand in for
// the verdict without also standing in for the prose that belongs to it, and
// that is a two-step restructure — a Noul for the verdict, a text model for
// the half the branch needs — which is worth doing once the ledger says the
// judgment is right often enough to pay for it.
//
// BUT ONE DIRECTION FALLS OUT OF THAT ARGUMENT ALREADY, and it is the one
// worth having. Going from VAGUE to CRISP throws away prose that is about to
// stop being needed: a run that proceeds asks nobody the questions, so nothing
// is discarded that anybody was going to read. The read — the scoper's own
// one- or two-sentence restatement — is written on both branches, so the run
// still has the sentence it plans against. `crisp_override` below is exactly
// that step, and nothing more.
//
// THE OTHER DIRECTION STAYS IMPOSSIBLE, for the reason the class argument
// gives: overriding a crisp verdict to vague would need the questions to ask,
// and a Noul answers yes or no. A run parked on an empty question list is a
// worse outcome than the one the scoper chose.
//
// Mode rides in the state because crispness is MODE-RELATIVE — "compare our
// two options" is a crisp recon and an underspecified expedition — and a
// judgment that cannot see the mode is answering a different question than the
// harness was asked.

use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Ask, Judgment, Question, ask_one, sites};

/// The census id this site records under.
pub const SITE: &str = "research-scope";

/// Judge whether an ask can be researched as typed.
///
/// Answers the judgment rather than a boolean, because the caller records it
/// against its own answer and the comparison is the whole point today.
pub async fn judge_crisp(
    state: &AppState,
    http: &HttpFetch,
    question: &str,
    mode: &str,
) -> Option<Judgment> {
    if question.trim().is_empty() {
        return None;
    }
    ask_one(
        state,
        http,
        serde_json::json!({
            "ask": question,
            // MODE-RELATIVE, and this is the field that makes the question
            // answerable at all: the same ask is crisp for a quick look and
            // underspecified for a deep one.
            "depth": mode,
        }),
        Question::noul_meaning(
            "Can the `ask` below be researched as written at the stated `depth`, without someone clarifying it first?",
            "it names what to find out clearly enough that a competent researcher would know where to start and when they were done",
            "it is too broad, too vague, or ambiguous between readings that would lead to different research — someone has to narrow it first",
        ),
    )
    .await
}

/// How the harness's own answer renders for the ledger, in the same
/// vocabulary `noul_agrees` reads: a Noul is compared as the boolean it would
/// gate on.
pub fn baseline_of(crisp: bool) -> String {
    crisp.to_string()
}

/// Ask the same question the scoper answered, over one state, and hand back
/// both halves ready to record. Separate from `judge_crisp` so a caller that
/// already holds an `Ask` is not forced to rebuild it.
pub fn ask_for(question: &str, mode: &str) -> Ask {
    Ask::new(serde_json::json!({ "ask": question, "depth": mode })).q(
        "crisp",
        Question::noul_meaning(
            "Can the `ask` below be researched as written at the stated `depth`, without someone clarifying it first?",
            "it names what to find out clearly enough that a competent researcher would know where to start and when they were done",
            "it is too broad, too vague, or ambiguous between readings that would lead to different research - someone has to narrow it first",
        ),
    )
}

/// MAY A RUN PROCEED ON AN ASK ITS OWN SCOPER CALLED VAGUE?
///
/// ONE DIRECTION, as the header says: `true` means "work the ask as typed and
/// ask nothing", and there is no answer this returns that sends a crisp ask
/// back for questions. The caller consults it only on the vague branch, so a
/// `false` is the branch it was already on.
///
/// Recording is NOT done here. The scoper's own pass records this site's
/// comparison against its verdict, detached, exactly as it did before this
/// function existed — a second write from the acting path would double every
/// row the agreement rate is computed over.
///
/// WHICH COSTS A SECOND JUDGMENT on the vague branch: the measurement asks
/// once for the ledger and this asks once to act. Named rather than optimised
/// away, because the alternative is for the acting path to own the row and for
/// the ledger's denominator to change meaning under a switch — and a Noul is
/// the cheapest call a research run makes, against a branch that was about to
/// stop and ask a person instead.
///
/// AND NOT AT ALL WITH THE SITE OFF, which is every install today: the switch
/// is read before the ask, because the ledger does not need this row — the
/// measurement next door writes it — and a judgment nobody will act on is a
/// call for nothing. `acted` still reads the switch after, so a flip between
/// the two reads lands on the safe side (the branch it was already on).
pub async fn crisp_override(
    state: &AppState,
    http: &HttpFetch,
    question: &str,
    mode: &str,
) -> bool {
    if !sites::gate_of(&state.pg, SITE).await.on {
        return false;
    }
    let judged = judge_crisp(state, http, question, mode).await;
    overrides(sites::acted(&state.pg, SITE, judged).await)
}

/// Does an ACTED judgment mean "proceed"?
///
/// The yes is read explicitly rather than taken from `acted` returning `Some`.
/// This site reads its floor as the probability of yes today, so the two agree
/// — but the reading is a census field and the floor is an operator's, and
/// under `FLOOR_CERTAINTY` a CONFIDENT NO clears it too. "Definitely vague"
/// must never be the answer that skips the questions.
fn overrides(judged: Option<Judgment>) -> bool {
    judged
        .and_then(|j| j.answer.probability())
        .is_some_and(|p| p >= 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judgment(p: f64, calibrated: bool) -> Judgment {
        Judgment {
            answer: crate::Answer::Noul(p),
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
    fn the_harness_answer_renders_in_the_vocabulary_the_comparator_reads() {
        // `noul_agrees` compares `(p >= 0.5) == (baseline == "true")`, so the
        // baseline has to be exactly that spelling or every row disagrees.
        assert_eq!(baseline_of(true), "true");
        assert_eq!(baseline_of(false), "false");
        assert!(crate::shadow::noul_agrees(
            &crate::Answer::Noul(0.9),
            &baseline_of(true)
        ));
        assert!(crate::shadow::noul_agrees(
            &crate::Answer::Noul(0.1),
            &baseline_of(false)
        ));
        assert!(!crate::shadow::noul_agrees(
            &crate::Answer::Noul(0.9),
            &baseline_of(false)
        ));
    }

    #[test]
    fn the_question_carries_the_depth_because_crispness_is_mode_relative() {
        // "compare our two options" is a crisp recon and an underspecified
        // expedition. A judgment that cannot see the depth is answering a
        // different question than the harness was asked.
        let ask = ask_for("compare our two options", "expedition");
        assert_eq!(ask.state["depth"], serde_json::json!("expedition"));
        assert_eq!(
            ask.state["ask"],
            serde_json::json!("compare our two options")
        );
        assert_eq!(ask.questions.len(), 1);
        let q = ask.questions.get("crisp").expect("one question, by id");
        assert_eq!(q.primitive(), "noul");
        assert!(q.wellformed());
        // Both meanings are spelled out: the chat wire builds its YES/NO menu
        // from them, so a question without them would send a different prompt
        // than the systemone wire does.
        let Question::Noul { yes_no, .. } = q else {
            panic!("a noul");
        };
        let (yes, no) = yes_no.as_ref().expect("yes and no are spelled out");
        assert!(yes.contains("where to start"), "{yes}");
        assert!(no.contains("narrow it first"), "{no}");
    }

    #[test]
    fn this_site_is_in_the_census_and_the_census_names_the_one_direction() {
        // Was "acts on nothing yet" until `crisp_override` shipped. The test
        // it replaces had the right question — the census must not describe
        // behaviour this site does not have — and now the lie runs the other
        // way: a site that can skip a person's clarifying questions and says
        // "Nothing yet" is the failure this module exists to prevent.
        let def = crate::sites::def_of(SITE).expect("research-scope is in the census");
        assert_eq!(def.primitive, "noul");
        // Still off on every install: skipping somebody's questions is a
        // deliberate act, taken with the sentence below in front of them.
        assert!(!def.default_on);
        assert!(
            !def.acts.contains("Nothing yet"),
            "this site acts now: {}",
            def.acts
        );
        assert!(
            def.acts.contains("One direction only"),
            "the census must say which way it can go: {}",
            def.acts
        );
    }

    #[test]
    fn only_a_yes_proceeds_and_a_confident_vague_never_does() {
        // THE ONE-DIRECTION RULE, pinned without a database. `overrides` is
        // the half that reads the answer; `acts_on` is the pure gate it is
        // handed, so the whole decision is exercised here bar the pool read.
        let def = crate::sites::def_of(SITE).expect("in the census");
        let gate = crate::SiteGate {
            on: true,
            floor: def.default_floor,
        };
        assert!(overrides(crate::sites::acts_on(
            def,
            &gate,
            Some(judgment(0.95, true))
        )));
        // A confident "vague" is the branch the caller was already on.
        assert!(!overrides(Some(judgment(0.02, true))));
        assert!(!overrides(crate::sites::acts_on(
            def,
            &gate,
            Some(judgment(0.02, true))
        )));
        // An uncalibrated yes acts on nothing, and neither does silence.
        assert!(!overrides(crate::sites::acts_on(
            def,
            &gate,
            Some(judgment(0.95, false))
        )));
        assert!(!overrides(None));
        // And with the site off — which is every install until somebody turns
        // it on — a confident yes still proceeds with nothing.
        let off = crate::SiteGate {
            on: false,
            floor: def.default_floor,
        };
        assert!(!overrides(crate::sites::acts_on(
            def,
            &off,
            Some(judgment(0.95, true))
        )));
    }
}

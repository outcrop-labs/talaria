// IS THE ASK CRISP ENOUGH TO RESEARCH — the research scoper's verdict half.
//
// The scoper runs before anything is planned or searched and answers
// `{crisp, read, questions}`: whether the ask can be worked as typed, one or
// two sentences saying it back, and — exactly when it is not crisp — the
// questions a person has to answer first. `crisp` is a boolean, so it is a
// Noul, and it arrives today as a JSON field a text model wrote with no number
// on it.
//
// WHY THIS IS MEASURED AND NOT ACTED ON, and it is the same structural reason
// as `action` next door — worth stating once about the class rather than twice
// about the instances. THE VERDICT IS COUPLED TO ITS PROSE. `questions` is
// non-empty exactly when `crisp` is false, and both halves are posted into the
// run's discussion for a person to read. So overriding `crisp` to true would
// discard questions somebody was about to be asked; overriding it to false
// would produce a vague verdict with nothing to ask. The typed answer cannot
// replace the verdict without also replacing the prose that belongs to it, and
// that is a two-step restructure — a Noul for the verdict, a text model for
// the half the branch needs — which is worth doing once the ledger says the
// judgment is right often enough to pay for it.
//
// So: the question is asked, recorded against the harness's own `crisp`, and
// nothing else. Mode rides in the state because crispness is MODE-RELATIVE —
// "compare our two options" is a crisp recon and an underspecified expedition
// — and a judgment that cannot see the mode is answering a different question
// than the harness was asked.

use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Ask, Judgment, Question, ask_one};

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn this_site_is_in_the_census_and_acts_on_nothing_yet() {
        let def = crate::sites::def_of(SITE).expect("research-scope is in the census");
        assert_eq!(def.primitive, "noul");
        assert!(!def.default_on);
        assert!(
            def.acts.contains("Nothing yet"),
            "the census must not promise behaviour this site does not have: {}",
            def.acts
        );
    }
}

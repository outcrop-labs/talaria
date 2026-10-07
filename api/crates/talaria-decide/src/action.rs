// WHICH ALLOWLISTED ACTION AN INSTRUCTION ASKS FOR — the inbox command's
// action selection, as a Choice over exactly the ids the owner authorized.
//
// WHY A CHOICE IS THE RIGHT SHAPE HERE, and what it actually buys. The inbox
// command harness returns `{message, actionId}` as JSON and
// `validate_command_object` is the authority gate: an `actionId` outside the
// allowlist is dropped to null. That gate is not negotiable and this does not
// touch it. But a Choice whose OPTIONS ARE THE ALLOWLIST cannot name something
// outside it at all — the check becomes a type — and the answer arrives with a
// calibrated number on it, which a JSON `actionId` never had.
//
// WHAT THE AUDIT GOT WRONG, worth recording because it changes what this is
// for. The plan described this as "a Choice over the allowed ids", assuming
// several. `allowed_focus_action_ids` returns the card's whole action list only
// for a WIDENED model; otherwise it returns at most one id (the deterministic
// proposal's), and in plan mode none at all. A Choice over one option is not a
// question — the port refuses it. So the real question is not "which of these
// several" but "this one, or none": whether the instruction asks for the
// authorized action at all, which is a two-option Choice that works for one
// allowlisted id and for five.
//
// SHADOW ONLY, and deliberately. The call site returns a `CommandTurn` whose
// `payload` belongs to its `action_id` — a `reply` carries the text to post —
// so swapping the id without swapping the payload would produce a proposal
// that does not match its own arguments. Measuring first is not caution here,
// it is the only order in which the acting version can be written correctly:
// the ledger says whether the judgment and the harness ever disagree, and on
// which ids, before anything is rewired.

use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Ask, Judgment, Opt, Question, decide};

/// The census id this site records under.
pub const SITE: &str = "action-select";

/// The option meaning "the instruction asks for none of these". Always
/// offered, so "do nothing" is a thing the model can SAY rather than something
/// inferred from a low probability — and so a one-id allowlist is still a
/// real question.
pub const NO_ACTION: &str = "none";

/// One action the owner has authorized on this card.
pub struct Offered {
    /// The id the harness would put in `actionId`.
    pub id: String,
    /// The label the person sees on the button, which is what the judgment
    /// reads — an id is a name, not an instruction.
    pub label: String,
    /// `safe` | `reversible` | `confirmation`, carried as given. In the state
    /// because "this one needs a confirmation" is part of what the instruction
    /// is or is not asking for.
    pub risk: String,
}

/// Judge which authorized action an instruction asks for.
///
/// Answers the judgment rather than an id, because the CALLER records it
/// against its own answer and the comparison is the whole point today. `None`
/// whenever the port has nothing, which is every install by default.
pub async fn judge_action(
    state: &AppState,
    http: &HttpFetch,
    instruction: &str,
    item_question: &str,
    offered: &[Offered],
) -> Option<Judgment> {
    if instruction.trim().is_empty() || offered.is_empty() {
        return None;
    }
    let mut options = vec![Opt::new(
        NO_ACTION,
        "none of them — the instruction is a question, a request for information, or asks for something this card cannot do, so nothing should be proposed",
    )];
    for a in offered {
        options.push(Opt::new(
            a.id.clone(),
            format!(
                "{} — the button labelled \"{}\"{}",
                a.id,
                a.label.trim(),
                match a.risk.as_str() {
                    "confirmation" => ", which asks the owner to confirm before it runs",
                    "reversible" => ", which can be undone",
                    _ => "",
                }
            ),
        ));
    }
    let ask = Ask::new(serde_json::json!({
        "instruction": instruction,
        // The card's own question — what this row is asking the person to
        // decide. Without it an instruction like "yes, do it" has no referent.
        "card_asks": item_question,
    }))
    .q(
        "action",
        Question::choice(
            "Which of the listed actions is the owner's `instruction` asking for? Choose an action only when the instruction plainly asks for that one; an instruction that asks a question, requests a summary, or is ambiguous between two of them asks for none.",
            options,
        ),
    );
    decide(state, http, &ask).await?.remove("action")
}

/// How this site renders the harness's own answer for the ledger: the
/// `actionId` it chose, or `none`. The same vocabulary the Choice answers in,
/// so the two columns are directly comparable.
pub fn baseline_of(action_id: Option<&str>) -> String {
    match action_id {
        Some(id) if !id.trim().is_empty() => id.trim().to_string(),
        _ => NO_ACTION.to_string(),
    }
}

/// Agreement for this site: the chosen id is the one the harness chose.
pub fn agrees(answer: &crate::Answer, baseline: &str) -> bool {
    answer.chosen() == Some(baseline)
}

/// The state and question are built from the card, so a caller that has no
/// instruction has nothing to ask — exported so the call site can skip the
/// whole pass without constructing anything.
pub fn worth_asking(instruction: &str, offered: &[Offered]) -> bool {
    !instruction.trim().is_empty() && !offered.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn offer(id: &str, label: &str, risk: &str) -> Offered {
        Offered {
            id: id.into(),
            label: label.into(),
            risk: risk.into(),
        }
    }

    #[test]
    fn a_single_allowlisted_action_is_still_a_real_question() {
        // THE FINDING THAT SHAPED THIS. `allowed_focus_action_ids` returns one
        // id for a non-widened model, and a Choice over one option is refused
        // by the port. Offering `none` alongside it is what makes "this one,
        // or nothing" askable — which is the question that was actually worth
        // asking all along.
        let options = vec![
            Opt::new(NO_ACTION, "none of them"),
            Opt::new(
                "approve_task",
                "approve_task — the button labelled \"Approve\"",
            ),
        ];
        let q = Question::choice("which one", options);
        assert!(
            q.wellformed(),
            "one authorized action plus none is two options"
        );
        assert_eq!(q.primitive(), "choice");
        // And with no allowlist at all there is nothing to ask about.
        assert!(!worth_asking("do it", &[]));
        assert!(!worth_asking("   ", &[offer("a", "A", "safe")]));
        assert!(worth_asking("approve it", &[offer("a", "A", "safe")]));
    }

    #[test]
    fn the_harness_answer_and_the_judgment_share_one_vocabulary() {
        // Two columns in the ledger have to mean the same thing or the
        // agreement rate is noise. A null `actionId` and the Choice's `none`
        // are the SAME answer and must render identically.
        assert_eq!(baseline_of(None), NO_ACTION);
        assert_eq!(baseline_of(Some("   ")), NO_ACTION);
        assert_eq!(baseline_of(Some("approve_task")), "approve_task");
        assert_eq!(baseline_of(Some("  approve_task  ")), "approve_task");
    }

    #[test]
    fn agreement_compares_the_chosen_id_including_when_both_say_none() {
        let choice = |id: &str| crate::Answer::Choice {
            id: id.into(),
            probabilities: BTreeMap::from([(id.to_string(), 1.0)]),
            confidence: 1.0,
        };
        assert!(agrees(&choice("approve_task"), "approve_task"));
        assert!(!agrees(&choice("approve_task"), NO_ACTION));
        // Both declining IS agreement — the common case, and leaving it out
        // would compute the rate over only the turns that proposed something.
        assert!(agrees(&choice(NO_ACTION), NO_ACTION));
        // A noul has no chosen option, so it can never agree here.
        assert!(!agrees(&crate::Answer::Noul(1.0), NO_ACTION));
    }

    #[test]
    fn this_site_is_in_the_census_and_acts_on_nothing_yet() {
        let def = crate::sites::def_of(SITE).expect("action-select is in the census");
        assert_eq!(def.primitive, "choice");
        assert_eq!(
            def.reads,
            crate::sites::FLOOR_CERTAINTY,
            "a choice has no probability of yes to lean on"
        );
        assert!(!def.default_on);
        // The census has to say that this one changes nothing yet, or an
        // operator reading the panel would think switching it on does
        // something.
        assert!(
            def.acts.contains("Nothing yet"),
            "the census must not promise behaviour this site does not have: {}",
            def.acts
        );
    }
}

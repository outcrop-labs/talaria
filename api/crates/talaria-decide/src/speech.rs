// WHETHER AN AGENT SPEAKS WHEN NOBODY ADDRESSED IT.
//
// THE ASYMMETRY THIS EXISTS TO FIX. In a ticket's task room the assigned agent
// answers whatever the relevance gate says is for it. In a group channel an
// agent speaks only when @mentioned. That second rule is deliberate — the
// alternative used to be chatter — but it means an agent that could answer the
// question in #eng sits there silently unless somebody remembers it exists.
// The gate was never affordable to run on every message in every channel. At a
// typed yes/no in under a tenth of a second it is.
//
// THIS IS THE HIGHEST-RISK SITE ON THE PORT and the design is shaped by the
// failure mode rather than by the feature:
//
//   AT MOST ONE AGENT SPEAKS. If three agents clear the floor on one
//   unaddressed message, three agents reply, and the room stops being usable
//   by people. The highest-probability candidate speaks and the rest stay
//   quiet. This is the difference between "more interactive" and "a room full
//   of bots", and it is not a tuning decision.
//
//   THE FLOOR IS HIGH AND LEANS. A confident NO means "stay quiet", not "say
//   the opposite", so the floor is read as the probability of yes — never as
//   distance from the middle, which would make a confident no an instruction
//   to speak.
//
//   EVERY ROOM CAN OPT OUT. `channels.agent_initiative`, checked by the
//   caller, because a room where this is wrong (#announcements, a customer
//   channel) must not need the capability turned off everywhere.
//
//   OFF ON EVERY INSTALL, like every site in the census.
//
// NOT A LOOP. An agent's reply is written through `insert_channel_message`
// rather than through the POST route, so it does not re-enter
// `trigger_agent_replies` and cannot answer itself. A person replying to an
// agent that spoke unprompted is a conversation, which is the point.

use serde_json::{Value, json};
use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Ask, Question, decide, sites};

/// The census id this site records and reads its switch under.
pub const SITE: &str = "channel-speech";

/// HOW MANY AGENTS ARE ASKED ABOUT. One Noul per candidate in a single
/// request, so the cost is one round trip — but a channel with thirty agents
/// in it would be thirty questions over one state, and past a handful the
/// answer to "which one of these should speak" is "a person should pick".
pub const MAX_CANDIDATES: usize = 8;

/// One agent that could speak, and what the judgment knows about it.
pub struct Candidate {
    /// The agent's model id — what the caller switches on.
    pub model: String,
    /// What this agent is for, in its own words. The ONLY thing that
    /// distinguishes one candidate from another, so an agent with no
    /// description cannot be judged and is not offered.
    pub about: String,
}

/// The candidates actually asked about: those that can be judged, capped.
///
/// An agent with no remit is DROPPED rather than asked about with an empty
/// description — a yes/no over a bare model id is a coin flip dressed as a
/// decision, the same rule the workflow pass holds for a hook with neither
/// name nor description. The cap takes the caller's own order, which is
/// deterministic, so which agents get asked does not vary between messages.
fn offered(candidates: &[Candidate]) -> Vec<&Candidate> {
    candidates
        .iter()
        .filter(|c| !c.about.trim().is_empty() && !c.model.trim().is_empty())
        .take(MAX_CANDIDATES)
        .collect()
}

/// The agent that should answer an unaddressed message, or `None`.
///
/// `None` means "nobody speaks", which is exactly what the code did before
/// this site existed — so with the port off, the site off, or every candidate
/// below the floor, a channel behaves as it always has.
///
/// Every judgment is recorded, including the ones below the floor: an
/// agreement rate over only the acted-on half is not one, and the baseline
/// here is always "false" because silence is what the existing code chose.
pub async fn should_speak(
    state: &AppState,
    http: &HttpFetch,
    subject: Value,
    candidates: &[Candidate],
) -> Option<String> {
    let gate = sites::gate_of(&state.pg, SITE).await;
    let def = sites::def_of(SITE)?;
    let offered = offered(candidates);
    if offered.is_empty() {
        return None;
    }
    let mut ask = Ask::new(subject);
    for c in &offered {
        ask = ask.q(
            c.model.clone(),
            Question::noul_meaning(
                format!(
                    "Should the agent described below join this conversation right now, although nobody addressed it? THE AGENT: {}",
                    c.about.trim()
                ),
                "the latest message is a question or a request this agent in particular can answer well, and answering it unprompted would help rather than interrupt",
                "nobody needs this agent here — the message is between people, is small talk, is addressed to someone else, is already answered, or is outside what this agent is for",
            ),
        );
    }
    let answers = decide(state, http, &ask).await?;

    let mut best: Option<(String, f64)> = None;
    for c in &offered {
        let Some(j) = answers.get(&c.model) else {
            continue;
        };
        crate::shadow::record(
            &state.pg,
            &crate::shadow::Compare {
                site: SITE,
                subject_ref: Some(c.model.clone()),
                // Silence is what the existing rule chose for every one of
                // these messages, so that is the baseline being compared
                // against — and it is why a high agreement rate here means
                // "it would rarely have spoken", not "it is right".
                baseline: "false".to_string(),
                agrees: crate::shadow::noul_agrees,
            },
            Some(j),
        )
        .await;
        if sites::acts_on(def, &gate, Some(j.clone())).is_none() {
            continue;
        }
        let p = j.answer.probability().unwrap_or(0.0);
        // AT MOST ONE. Ties go to the first candidate in the caller's own
        // order, which is deterministic — two agents with identical
        // probabilities must not take turns at random.
        if best.as_ref().is_none_or(|(_, bp)| p > *bp) {
            best = Some((c.model.clone(), p));
        }
    }
    best.map(|(model, _)| model)
}

/// The state the judgment reads: the room, who just spoke, what they said, and
/// the turns before it.
///
/// WHAT IS NOT HERE: the agent's own description. That is in the QUESTION, one
/// per candidate, because the state is shared across every question in the
/// request and an agent's remit is what distinguishes its question from its
/// neighbour's.
pub fn subject(channel: &str, sender: &str, message: &str, recent: &[String]) -> Value {
    json!({
        "channel": channel,
        "from": sender,
        "message": message,
        // Empty-as-empty: "nothing was said before this" is a fact the
        // judgment turns on — a cold open in a quiet room is a different
        // thing from a reply in a running thread.
        "recent": recent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_site_is_in_the_census_and_leans_because_a_confident_no_means_stay_quiet() {
        let def = sites::def_of(SITE).expect("channel-speech is in the census");
        assert_eq!(
            def.reads,
            sites::FLOOR_LEAN,
            "read as certainty, a confident NO would clear the floor and the agent would speak"
        );
        assert!(!def.default_on, "off on every install");
        // The highest floor on the port, because the failure mode is an agent
        // interrupting people in a shared room.
        for other in sites::DECIDE_SITES.iter().filter(|s| s.id != SITE) {
            assert!(
                def.default_floor >= other.default_floor,
                "{} has a higher floor than unprompted speech",
                other.id
            );
        }
    }

    fn cand(model: &str, about: &str) -> Candidate {
        Candidate {
            model: model.into(),
            about: about.into(),
        }
    }

    #[test]
    fn an_agent_with_nothing_to_say_about_itself_is_not_asked_about() {
        // The remit is the ONLY thing that distinguishes one candidate's
        // question from another's, so a blank one would be a coin flip dressed
        // as a decision.
        let cands = [
            cand("a", "   "),
            cand("", "fine"),
            cand("c", "Support — answers billing questions"),
        ];
        let got = offered(&cands);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model, "c");
        assert!(offered(&[]).is_empty());
    }

    #[test]
    fn the_subject_names_the_room_and_keeps_an_empty_history_empty() {
        let s = subject("#eng", "Dana", "anyone know why staging is 500ing?", &[]);
        assert_eq!(s["channel"], json!("#eng"));
        assert_eq!(s["from"], json!("Dana"));
        assert!(
            s["message"].as_str().is_some_and(|m| m.contains("staging")),
            "the message being judged has to actually be in the state"
        );
        // Not omitted: a cold open in a quiet room is a different thing from a
        // reply in a running thread, and an absent key reads as unknown.
        assert_eq!(s["recent"], json!([]));
        let with = subject("#eng", "Dana", "and now?", &["user: hello".to_string()]);
        assert_eq!(with["recent"], json!(["user: hello"]));
    }

    #[test]
    fn a_room_full_of_agents_is_capped_and_capped_deterministically() {
        // One question per candidate in one request, so this is a cost — and
        // past a handful the answer to "which of these should speak" is "a
        // person should pick". Asserted as BEHAVIOUR over twenty candidates
        // rather than by comparing the constant with a literal, which is a
        // compile-time tautology clippy rightly rejects.
        let many: Vec<Candidate> = (0..20)
            .map(|i| cand(&format!("m{i}"), &format!("Agent {i} — does thing {i}")))
            .collect();
        let got = offered(&many);
        assert!(
            got.len() < many.len(),
            "twenty agents must not all be asked"
        );
        assert!(got.len() >= 2, "one candidate is not a choice");
        // The caller's own order, so which agents are asked does not vary
        // between two messages in the same room.
        assert_eq!(got[0].model, "m0");
        assert_eq!(got[1].model, "m1");
        assert_eq!(offered(&many).len(), got.len());
    }
}

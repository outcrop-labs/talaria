// THE DECISION PORT — one typed judgment, from whichever decision model the
// operator pointed us at.
//
// WHY THIS CRATE EXISTS
//   Talaria asks a text model to DECIDE things in a dozen places, and every
//   one of them pays the same tax: a prompt that spells out the answer shape
//   in English, a JSON parse, a repair turn when the parse fails, a
//   normalizer folding "yes"/"true"/"relevant" back into a boolean, and — at
//   the end of all that — a bare answer with no number on it. The ticket
//   gate's uncertainty is a sentence in its prompt ("when genuinely
//   uncertain, answer true"); a guard finding's `confidence` is a constant a
//   developer typed; a workflow either matched a substring or did not.
//
//   A decision model answers the shape by construction and hands back a
//   probability. That is the whole of what this port buys: the answer is a
//   type, and the uncertainty is a number a threshold can be chosen against.
//
// IT IS A REGISTRY, NOT AN INTEGRATION. An operator runs whichever decision
// model they want — a hosted one, a classifier on their own GPU, a chat model
// they already registered, or something that does not exist yet. The shipped
// adapters are a convenience and never a ceiling: `custom` takes a URL and a
// declaration of which wire shape it speaks, so a provider we have never
// heard of needs no code from us. There is no blessed default and no
// recommended model — `config.rs` carries that reasoning in full.
//
// THE CONTRACT IS `Option`, AND THAT IS DELIBERATE. `decide` answers
// `Option<_>`, and `None` means "run your own path" — exactly the shape
// `rerank` already uses for the same reason. The port cannot run the call
// site's fallback: only the caller knows whether the deterministic answer is
// a keyword match, a slug comparison, or a costed fail-open. So the port
// never decides what a failure MEANS; it reports that it has nothing, and the
// call site keeps the branch it already had.
//
//   The property that follows is the one that makes this safe to ship: with
//   the port off, misconfigured, unreachable, or answering below a site's
//   threshold, every call site runs exactly the code it ran before this crate
//   existed. No call site may be written such that turning the port off makes
//   it worse than it was.
//
// CALIBRATION IS PART OF THE ANSWER, not an assumption about the provider. A
// `Judgment` carries `calibrated`, and it is false whenever the number behind
// the answer is not a real distribution — a chat model that dropped
// `logprobs`, a provider that answered without one. `gated` refuses to let a
// threshold read a number that is not there, because a fabricated confidence
// is worse than no confidence: it looks like evidence.

use std::collections::BTreeMap;

use serde_json::Value;
use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

pub mod config;
pub mod focus;
pub mod guard;
pub mod shadow;
pub mod sites;
pub mod speech;
pub mod tools;
mod wire;

pub use sites::{DECIDE_SITES, SiteDef, SiteGate, acted, gate_of, sites_public};
pub use wire::ModelInfo;

pub use config::{
    ALL_WIRES, DECIDE_PROVIDERS, DecideConfig, DecidePatch, DecideProviderMeta,
    DecideProviderPublic, configured, decide_config_public, get_decide_config, providers_public,
    set_decide_config,
};

// ── The question ─────────────────────────────────────────────────────────────

/// One option a `Choice` may answer with. `id` is what code switches on;
/// `description` is what the model reads, and it is the only thing that
/// decides the answer — an id is a label, not an instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct Opt {
    pub id: String,
    pub description: String,
}

impl Opt {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Opt {
        Opt {
            id: id.into(),
            description: description.into(),
        }
    }
}

/// What a question asks. The three primitives every System One model serves,
/// chosen by what the ANSWER MEANS rather than by how the prompt reads:
///
///   `Noul`    does this condition hold?            → a probability
///   `Choice`  which one of these?                  → one id + a distribution
///   `Score`   where on this described scale?       → a position + a distribution
///
/// The distinction that actually bites: a `Noul` near 0.5 means "as likely yes
/// as no", NOT "medium intensity". A question about degree is a `Score`.
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    Noul {
        instructions: String,
        /// What yes and what no MEAN here. Optional, and worth writing
        /// whenever the condition has an edge a reader would argue about.
        yes_no: Option<(String, String)>,
    },
    Choice {
        instructions: String,
        /// At least two. A `no_match` option belongs here whenever nothing
        /// may fit — the model cannot choose an answer it was not offered.
        options: Vec<Opt>,
    },
    Score {
        instructions: String,
        /// 2–10 levels, lowest first, each describing a concrete situation
        /// that stands on its own. "Medium" is not a level; "frustrated but
        /// civil" is.
        levels: Vec<String>,
    },
}

impl Question {
    pub fn noul(instructions: impl Into<String>) -> Question {
        Question::Noul {
            instructions: instructions.into(),
            yes_no: None,
        }
    }

    /// The same question with yes and no spelled out.
    pub fn noul_meaning(
        instructions: impl Into<String>,
        yes: impl Into<String>,
        no: impl Into<String>,
    ) -> Question {
        Question::Noul {
            instructions: instructions.into(),
            yes_no: Some((yes.into(), no.into())),
        }
    }

    pub fn choice(instructions: impl Into<String>, options: Vec<Opt>) -> Question {
        Question::Choice {
            instructions: instructions.into(),
            options,
        }
    }

    pub fn score(instructions: impl Into<String>, levels: Vec<String>) -> Question {
        Question::Score {
            instructions: instructions.into(),
            levels: levels.into_iter().collect(),
        }
    }

    /// The primitive's wire name — also the key `DecideProviderMeta.primitives`
    /// is matched against, so a provider that cannot serve a shape refuses
    /// before a request is built rather than after one fails.
    pub fn primitive(&self) -> &'static str {
        match self {
            Question::Noul { .. } => "noul",
            Question::Choice { .. } => "choice",
            Question::Score { .. } => "score",
        }
    }

    /// Is this question answerable as posed? A `Choice` with one option has
    /// one answer and does not need a model; a `Score` outside 2..=10 levels
    /// is outside every provider's contract.
    fn wellformed(&self) -> bool {
        match self {
            Question::Noul { instructions, .. } => !instructions.trim().is_empty(),
            Question::Choice {
                instructions,
                options,
            } => {
                !instructions.trim().is_empty()
                    && options.len() >= 2
                    && options.len() <= 255
                    && options.iter().all(|o| !o.id.trim().is_empty())
            }
            Question::Score {
                instructions,
                levels,
            } => {
                !instructions.trim().is_empty()
                    && (2..=10).contains(&levels.len())
                    && levels.iter().all(|l| !l.trim().is_empty())
            }
        }
    }
}

/// The state plus the questions asked over it. Independent questions belong in
/// ONE ask: a provider that fans out answers them in a single round trip and
/// they cannot see one another's answers anyway. A question that needs an
/// earlier answer — to fetch evidence, or to decide what the options are — is
/// a second ask.
#[derive(Debug, Clone)]
pub struct Ask {
    /// A string, a JSON object, or an array of strings. Prefer an object with
    /// named fields once the state has more than one part, so a question can
    /// refer to `ticket.description` and mean something.
    pub state: Value,
    /// Question id → question. Ids are for code and are never sent, so the
    /// question text must carry its whole meaning on its own.
    pub questions: BTreeMap<String, Question>,
}

impl Ask {
    pub fn new(state: Value) -> Ask {
        Ask {
            state,
            questions: BTreeMap::new(),
        }
    }

    pub fn q(mut self, id: impl Into<String>, question: Question) -> Ask {
        self.questions.insert(id.into(), question);
        self
    }
}

// ── The answer ───────────────────────────────────────────────────────────────

/// What came back, typed by the primitive that was asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Probability the condition holds, 0..=1.
    Noul(f64),
    Choice {
        /// The chosen option's id — always one of the ids that were offered.
        id: String,
        /// id → probability, over the offered options.
        probabilities: BTreeMap<String, f64>,
        /// How concentrated the distribution is, 0..=1.
        confidence: f64,
    },
    Score {
        /// The probability-weighted position, 0-indexed over the levels, so a
        /// 3-level question answers somewhere in 0.0..=2.0 and may land
        /// between levels.
        position: f64,
        /// Per-level probability, lowest level first.
        probabilities: Vec<f64>,
        confidence: f64,
    },
}

impl Answer {
    /// HOW SURE, on one 0..=1 axis, whichever primitive was asked — the number
    /// a threshold reads.
    ///
    /// For `Choice`/`Score` it is the provider's own confidence. For `Noul`
    /// there IS no provider confidence (the probability is the whole answer),
    /// so it is the distance from the coin flip, doubled: p=0.5 → 0.0,
    /// p=0.95 or p=0.05 → 0.9. A confident NO is as certain as a confident
    /// yes, which is the property a gate needs and the reason this is not
    /// simply `p`.
    pub fn certainty(&self) -> f64 {
        match self {
            Answer::Noul(p) => (p - 0.5).abs() * 2.0,
            Answer::Choice { confidence, .. } | Answer::Score { confidence, .. } => *confidence,
        }
    }

    /// The `Noul` probability, or None for the other two.
    pub fn probability(&self) -> Option<f64> {
        match self {
            Answer::Noul(p) => Some(*p),
            _ => None,
        }
    }

    /// The chosen `Choice` id, or None for the other two.
    pub fn chosen(&self) -> Option<&str> {
        match self {
            Answer::Choice { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }

    /// The `Score` position, or None for the other two.
    pub fn position(&self) -> Option<f64> {
        match self {
            Answer::Score { position, .. } => Some(*position),
            _ => None,
        }
    }
}

/// One answer plus what is known about how much to trust it.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgment {
    pub answer: Answer,
    /// IS THE NUMBER REAL? False when the provider answered without a usable
    /// distribution behind it — a chat model whose endpoint dropped
    /// `logprobs`, or any reply we had to assign a probability to rather than
    /// read one from. An uncalibrated judgment is still a usable ANSWER; what
    /// it is not is evidence about its own correctness, so `gated` refuses it.
    pub calibrated: bool,
    /// Which provider answered, for the ledger and the drill-down.
    pub provider: String,
    pub model: Option<String>,
    pub latency_ms: u64,
}

impl Judgment {
    pub fn certainty(&self) -> f64 {
        self.answer.certainty()
    }
}

/// THE CONFIDENCE GATE. `Some` only when all three hold: a judgment arrived,
/// the number behind it is real, and it clears `min_certainty`.
///
/// Threshold choice belongs to the CALL SITE, from what a wrong answer costs
/// there — the gate on a ticket thread can afford to be wrong and fall open;
/// a verdict that moves a ticket cannot. Numbers published in a vendor's
/// cookbook are examples to evaluate against our own traffic, never values to
/// copy.
pub fn gated(j: Option<Judgment>, min_certainty: f64) -> Option<Judgment> {
    let j = j?;
    if !j.calibrated || j.certainty() < min_certainty {
        return None;
    }
    Some(j)
}

// ── Why there is no answer ───────────────────────────────────────────────────

/// WHY THE PORT HAS NOTHING — and why this is not in `decide`'s return type.
///
/// `decide` answers `Option` on purpose (see its doc): a call site does the
/// same thing in every one of these cases, and one that branched on the reason
/// would be making a policy decision the port does not own. That stays true.
/// This type exists for ONE caller, the admin panel's Test button, whose whole
/// job is to tell a person which of these it is.
///
/// It exists because of a real afternoon. A key was pasted in correctly and
/// `model` was typed as `jev` rather than `jev-latest`; the panel answered
/// "the provider did not answer, or answered in a shape this wire does not
/// recognize", and the body sitting unread in the response said
/// `Unknown model: jev`. The provider's own sentence is worth more than any
/// sentence we can write, because it is about the configuration that is
/// actually there rather than about the ones we imagined.
#[derive(Debug, Clone, PartialEq)]
pub enum NoAnswer {
    /// No provider chosen, or one missing a field its registry entry says it
    /// needs.
    NotConfigured,
    /// Complete, and still not dispatchable as written — or a question this
    /// provider's capability sheet says it cannot answer. Carries the sentence.
    Unusable(String),
    /// The request never reached the endpoint: DNS, TLS, refused, timed out.
    Unreachable(String),
    /// It answered, and refused — `message` is the ENDPOINT'S own words.
    Refused { status: u16, message: String },
    /// A 2xx whose body this wire does not recognize as answers.
    Unreadable,
}

impl NoAnswer {
    /// A stable tag for the panel to branch on, so the sentence can be
    /// rewritten without breaking the UI.
    pub fn kind(&self) -> &'static str {
        match self {
            NoAnswer::NotConfigured => "not-configured",
            NoAnswer::Unusable(_) => "unusable",
            NoAnswer::Unreachable(_) => "unreachable",
            NoAnswer::Refused { .. } => "refused",
            NoAnswer::Unreadable => "unreadable",
        }
    }

    /// One sentence for a person. The refused case leads with the endpoint's
    /// own message and names the status after it: the status is context, the
    /// message is the answer.
    pub fn sentence(&self) -> String {
        match self {
            NoAnswer::NotConfigured => "No decision model is configured yet.".into(),
            NoAnswer::Unusable(why) => why.clone(),
            NoAnswer::Unreachable(why) => {
                format!("The endpoint could not be reached: {why}")
            }
            NoAnswer::Refused { status, message } => {
                format!("The provider refused ({status}): {message}")
            }
            NoAnswer::Unreadable => "The provider answered, but in a shape this wire does not recognize. Check that the URL points at the right protocol and that the model serves typed judgments.".into(),
        }
    }
}

// ── Asking ───────────────────────────────────────────────────────────────────

/// Ask one question and take the judgment. The common case; `decide` is the
/// fan-out.
pub async fn ask_one(
    state: &AppState,
    http: &HttpFetch,
    subject: Value,
    question: Question,
) -> Option<Judgment> {
    let ask = Ask::new(subject).q("q", question);
    decide(state, http, &ask).await?.remove("q")
}

/// Ask every question in `ask` over its state, and answer by question id.
///
/// `None` — the port has nothing and the caller runs its own path — covers
/// every one of: the provider is `off`, the config is incomplete, the provider
/// cannot serve a primitive that was asked, a question is malformed, the
/// endpoint refused or timed out, and the reply did not carry the answers.
/// Those are deliberately NOT distinguished in the return type: a call site
/// does the same thing in all of them, and a caller that branched on the
/// reason would be making a policy decision the port does not own.
///
/// A PARTIAL ANSWER IS AN ANSWER. When a provider answers four of five
/// questions, the four are returned and the fifth is simply absent from the
/// map. A caller reads the ids it needs and falls back per id, because the
/// alternative — discarding four good answers over one missing one — throws
/// away work that was already paid for.
pub async fn decide(
    state: &AppState,
    http: &HttpFetch,
    ask: &Ask,
) -> Option<BTreeMap<String, Judgment>> {
    try_decide(state, http, ask).await.ok()
}

/// `decide`, keeping the reason. The admin panel's door and nothing else's —
/// a call site that branches on `NoAnswer` is choosing a policy the port does
/// not own, and `decide` above is the one-line collapse that enforces that.
pub async fn try_decide(
    state: &AppState,
    http: &HttpFetch,
    ask: &Ask,
) -> Result<BTreeMap<String, Judgment>, NoAnswer> {
    if ask.questions.is_empty() {
        return Err(NoAnswer::Unusable("Nothing was asked.".into()));
    }
    if let Some((id, _)) = ask.questions.iter().find(|(_, q)| !q.wellformed()) {
        return Err(NoAnswer::Unusable(format!(
            "The question \"{id}\" is malformed — a judgment needs instructions and, for a choice or a score, at least two things to choose between."
        )));
    }
    let cfg = get_decide_config(&state.pg).await;
    let Some(meta) = config::meta_of(&cfg.provider) else {
        return Err(NoAnswer::NotConfigured);
    };
    // A primitive the provider does not serve refuses HERE, before a request
    // is built — the alternative is a 422 from a vendor, or worse, a
    // classifier quietly answering a Score question as a two-way split.
    if let Some((_, q)) = ask
        .questions
        .iter()
        .find(|(_, q)| !meta.primitives.contains(&q.primitive()))
    {
        return Err(NoAnswer::Unusable(format!(
            "{} does not answer {} questions — it serves {}.",
            meta.label,
            q.primitive(),
            meta.primitives.join(", ")
        )));
    }
    // A provider that cannot fan out answers one question per round trip.
    // Asking it five is five calls, which is a cost the CALLER should have
    // chosen, so the port refuses rather than quietly spending it.
    if !meta.fans_out && ask.questions.len() > 1 {
        return Err(NoAnswer::Unusable(format!(
            "{} answers one question per request, and {} were asked in one.",
            meta.label,
            ask.questions.len()
        )));
    }
    let out = wire::dispatch(state, http, &cfg, meta, ask).await?;
    if out.is_empty() {
        return Err(NoAnswer::Unreadable);
    }
    Ok(out)
}

/// The models this endpoint says it will accept in `model`, asked of the
/// endpoint itself. See `wire::list_models` for what each wire is asked and
/// why the registered-model provider is answered without a request at all.
pub async fn list_models(state: &AppState, http: &HttpFetch) -> Result<Vec<ModelInfo>, NoAnswer> {
    let cfg = get_decide_config(&state.pg).await;
    let Some(meta) = config::meta_of(&cfg.provider) else {
        return Err(NoAnswer::NotConfigured);
    };
    wire::list_models(state, http, &cfg, meta).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<Opt> {
        vec![Opt::new("a", "the first"), Opt::new("b", "the second")]
    }

    #[test]
    fn a_noul_certainty_is_the_distance_from_the_coin_flip_so_a_confident_no_counts() {
        assert_eq!(Answer::Noul(0.5).certainty(), 0.0);
        assert!((Answer::Noul(0.95).certainty() - 0.9).abs() < 1e-9);
        // THE POINT OF THE DOUBLING: a 5% yes is a 95% no, and a gate reading
        // `p` alone would have called it maximally uncertain.
        assert!((Answer::Noul(0.05).certainty() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn choice_and_score_certainty_is_the_providers_own_confidence_not_a_derivation() {
        let c = Answer::Choice {
            id: "a".into(),
            probabilities: BTreeMap::new(),
            confidence: 0.42,
        };
        assert_eq!(c.certainty(), 0.42);
        let s = Answer::Score {
            position: 1.5,
            probabilities: vec![0.1, 0.4, 0.5],
            confidence: 0.77,
        };
        assert_eq!(s.certainty(), 0.77);
    }

    fn judgment(answer: Answer, calibrated: bool) -> Judgment {
        Judgment {
            answer,
            calibrated,
            provider: "jev".into(),
            model: None,
            latency_ms: 1,
        }
    }

    #[test]
    fn the_gate_refuses_an_uncalibrated_judgment_however_certain_it_claims_to_be() {
        // 0.99 certainty and no real distribution behind it. Letting this
        // through is how a fabricated number becomes evidence.
        let j = judgment(Answer::Noul(0.995), false);
        assert!(gated(Some(j), 0.5).is_none());
    }

    #[test]
    fn the_gate_refuses_below_the_threshold_and_passes_at_it() {
        let at = judgment(Answer::Noul(0.75), true); // certainty 0.5
        assert!(gated(Some(at), 0.5).is_some());
        let below = judgment(Answer::Noul(0.74), true); // certainty 0.48
        assert!(gated(Some(below), 0.5).is_none());
    }

    #[test]
    fn the_gate_passes_nothing_through_when_there_was_no_judgment() {
        assert!(gated(None, 0.0).is_none());
    }

    #[test]
    fn a_choice_needs_two_real_options_because_one_option_is_not_a_question() {
        assert!(Question::choice("pick", opts()).wellformed());
        assert!(!Question::choice("pick", vec![Opt::new("a", "only")]).wellformed());
        assert!(!Question::choice("", opts()).wellformed());
        // An id nothing can switch on.
        assert!(
            !Question::choice("pick", vec![Opt::new(" ", "x"), Opt::new("b", "y")]).wellformed()
        );
    }

    #[test]
    fn a_score_holds_two_to_ten_levels_which_is_every_providers_contract() {
        assert!(!Question::score("how much", vec!["only".into()]).wellformed());
        assert!(Question::score("how much", vec!["low".into(), "high".into()]).wellformed());
        let ten: Vec<String> = (0..10).map(|i| format!("level {i}")).collect();
        assert!(Question::score("how much", ten).wellformed());
        let eleven: Vec<String> = (0..11).map(|i| format!("level {i}")).collect();
        assert!(!Question::score("how much", eleven).wellformed());
    }

    #[test]
    fn a_noul_needs_instructions_since_the_id_is_never_sent_to_the_model() {
        assert!(Question::noul("is it raining").wellformed());
        assert!(!Question::noul("   ").wellformed());
    }

    #[test]
    fn the_primitive_name_is_what_a_providers_capability_sheet_is_matched_against() {
        assert_eq!(Question::noul("x").primitive(), "noul");
        assert_eq!(Question::choice("x", opts()).primitive(), "choice");
        assert_eq!(
            Question::score("x", vec!["a".into(), "b".into()]).primitive(),
            "score"
        );
        // Every primitive a question can be must be a primitive a provider can
        // declare, or `decide` would refuse a well-formed ask forever.
        for p in ["noul", "choice", "score"] {
            assert!(
                DECIDE_PROVIDERS.iter().any(|m| m.primitives.contains(&p)),
                "no provider serves {p}"
            );
        }
    }

    #[test]
    fn an_ask_builds_by_id_and_a_repeated_id_replaces_rather_than_duplicating() {
        let ask = Ask::new(serde_json::json!({"t": "x"}))
            .q("one", Question::noul("a"))
            .q("two", Question::noul("b"))
            .q("one", Question::noul("c"));
        assert_eq!(ask.questions.len(), 2);
        assert_eq!(
            ask.questions.get("one"),
            Some(&Question::noul("c")),
            "the later question wins"
        );
    }
}

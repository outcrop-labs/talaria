// JUDGED ATTENTION ORDER — the brief's tiebreak, asked rather than assumed.
//
// WHAT IS WRONG WITH THE SORT IT REPLACES. A focus queue orders by
// `(bucket, due-ness, explicit priority, age)`, lexicographically. The BUCKET
// is sound: failed work outranks a waiting message because those are different
// CATEGORIES of thing, not different intensities, and nothing a model says
// should move a failed ticket below a notification. The three fields after it
// are a tiebreak by accident. "Due in six days, high priority, filed Tuesday"
// versus "due in eight days, urgent, filed this morning" is decided by which
// field happens to come first in the comparator, and the answer a person would
// give has nothing to do with that ordering.
//
// A Score is exactly that question: a probability-weighted position on ordered
// levels, one per item, all in one request over shared state. TypeSafe
// publishes it as composite scoring, and the shape fits without adaptation.
//
// WHAT THIS MODULE IS AND IS NOT. It is the question and the reading, generic
// over the item type — a `(key, state)` in, a judged urgency per key out. The
// BLEND is the caller's, and must be, because only the caller knows its
// buckets. Two crates hold a focus sort (`talaria-daily-brief-focus` for the
// daily brief, `talaria-inbox-focus::policy` for the live queue) over two
// different item types; a question built in each is how the two come to judge
// different things while claiming to judge one.
//
// THE RULE THE CALLER MUST HOLD, and the reason it is written here rather than
// only there: JUDGE WITHIN A BUCKET, NEVER ACROSS ONE. Reordering across
// buckets would let a confident model drop a failed deploy below an unread
// mention, and no threshold makes that acceptable — the buckets are a policy
// decision a person already made.

use std::collections::BTreeMap;

use serde_json::Value;
use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Answer, Ask, Question, decide, sites};

/// The census id this site records and reads its switch under.
pub const SITE: &str = "focus-rank";

/// FIVE CONCRETE SITUATIONS, lowest first. Each one has to stand on its own —
/// a level that reads as "medium" tells the model nothing, and a scale of
/// adverbs gets answered as a vibe. They describe who is waiting and what
/// happens if the item slides, because that is what "needs attention" means
/// on a page about what to do today.
pub const LEVELS: [&str; 5] = [
    "nothing is waiting on this. It is informational, already handled, or somebody else owns the next step.",
    "it will need attention eventually, but nothing changes if it waits a week.",
    "it should be handled this week. Letting it slide starts to cost something — a slipping date, a reply someone is mildly waiting for.",
    "it needs attention today. Somebody or something is held up until it is dealt with.",
    "it is the first thing to do. Work is stopped, a commitment is at risk, or a person is waiting on an answer right now.",
];

/// One item to be ranked: the key code switches on, and the state the question
/// reads. The caller builds the state, because the fields worth judging are
/// its own — a title, the question the row poses, its evidence, its due date.
pub struct Candidate {
    pub key: String,
    pub state: Value,
}

/// HOW MANY ROWS ARE JUDGED, and why there is a cap at all. A brief's sources
/// return up to 200 rows each; one Score per row over shared state would push
/// a single request past any provider's context window, and the tail of a
/// brief is not what an ordering is for — nobody reads row 90 and wonders
/// whether it should have been row 85. The top of the deterministic order is
/// judged and the rest keeps it.
pub const MAX_JUDGED: usize = 40;

/// What the port thinks, and whether this site is switched on to use it.
pub struct Ranking {
    /// Judged urgency per key, 0.0..=1.0, where 1.0 is "the first thing to
    /// do". Only keys with a USABLE answer appear: one that arrived, carried a
    /// real distribution, and cleared this site's floor.
    pub urgency: BTreeMap<String, f64>,
    /// Is the site switched on? When `false` the caller computes the order it
    /// WOULD have produced, records the comparison, and then uses the
    /// deterministic one — which is shadow mode, and the reason the question
    /// is asked even with the site off.
    pub acts: bool,
}

/// Ask for an urgency per item.
///
/// ASKED WHETHER OR NOT THE SITE IS SWITCHED ON, which is deliberate and is
/// the same bargain the ticket gate makes: with the site off this is pure
/// measurement, so an operator can read how often the judged order would have
/// differed before trusting it. The cost is real — one request per brief
/// against a configured provider — and it is the price of not choosing a
/// threshold by guess. An install with no provider configured pays nothing:
/// `decide` answers `None` before a request is built.
///
/// `items` must arrive in the DETERMINISTIC order. Two things depend on it:
/// the cap takes the top of that order, and `reordered` uses the incoming
/// sequence as its tiebreak.
pub async fn rank(state: &AppState, http: &HttpFetch, today: &str, items: &[Candidate]) -> Ranking {
    let gate = sites::gate_of(&state.pg, SITE).await;
    let acts = gate.on;
    let Some(def) = sites::def_of(SITE) else {
        return Ranking {
            urgency: BTreeMap::new(),
            acts: false,
        };
    };
    let judged = &items[..items.len().min(MAX_JUDGED)];
    if judged.len() < 2 {
        // One row has no order to get wrong, and zero rows is not a brief.
        return Ranking {
            urgency: BTreeMap::new(),
            acts,
        };
    }
    // `today` is in the state because a due date is uninterpretable without
    // it, and a model's own idea of the date is not a fact about this install.
    let mut ask = Ask::new(serde_json::json!({
        "today": today,
        "items": judged.iter().map(|c| &c.state).collect::<Vec<_>>(),
    }));
    for (i, c) in judged.iter().enumerate() {
        ask = ask.q(
            c.key.clone(),
            Question::score(
                format!(
                    "How much does `items[{i}]` need this person's attention today? Judge the item at that index only; the others are context for comparing it against."
                ),
                LEVELS.iter().map(|l| (*l).to_string()).collect(),
            ),
        );
    }
    let Some(answers) = decide(state, http, &ask).await else {
        return Ranking {
            urgency: BTreeMap::new(),
            acts,
        };
    };
    let mut urgency = BTreeMap::new();
    for c in judged {
        let Some(j) = answers.get(&c.key) else {
            continue;
        };
        if sites::acts_on(def, &gate, Some(j.clone())).is_none() {
            continue;
        }
        let Answer::Score { position, .. } = j.answer else {
            continue;
        };
        urgency.insert(c.key.clone(), normalized(position));
    }
    Ranking { urgency, acts }
}

/// A weighted position over `LEVELS` as a 0..=1 urgency. Divided by the number
/// of GAPS rather than of levels, so the top level is exactly 1.0 — and
/// clamped, because a provider answering outside its own range must not sort
/// an item off the end.
fn normalized(position: f64) -> f64 {
    if !position.is_finite() {
        return 0.0;
    }
    (position / (LEVELS.len() - 1) as f64).clamp(0.0, 1.0)
}

/// The order a set of keys takes once judged, most urgent first, with the
/// caller's own order as the tiebreak for equal urgency.
///
/// `keys` arrives in the deterministic order, and that is what makes this
/// safe: two items the model scored identically keep the sequence the existing
/// policy gave them, so the judgment only ever breaks ties the old comparator
/// was breaking arbitrarily.
pub fn reordered(keys: &[String], urgency: &BTreeMap<String, f64>) -> Option<Vec<String>> {
    // ALL OR NOTHING. A bucket ordered half by judgment and half by age is
    // ordered by neither, and the result would look like a bug in whichever
    // one the reader expected.
    if keys.is_empty() || !keys.iter().all(|k| urgency.contains_key(k)) {
        return None;
    }
    let mut out: Vec<(usize, &String)> = keys.iter().enumerate().collect();
    out.sort_by(|(ia, a), (ib, b)| {
        let (ua, ub) = (urgency.get(*a).copied(), urgency.get(*b).copied());
        ub.partial_cmp(&ua)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ia.cmp(ib))
    });
    Some(out.into_iter().map(|(_, k)| k.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_levels_are_a_well_formed_score_question() {
        // 2..=10 is every provider's contract, and the question is refused
        // before a request is built outside it.
        let q = Question::score(
            "how urgent",
            LEVELS.iter().map(|l| (*l).to_string()).collect(),
        );
        assert!(q.wellformed());
        assert_eq!(q.primitive(), "score");
        for l in LEVELS {
            // A level that reads as "medium" tells the model nothing. Each one
            // has to describe a situation, which takes a sentence.
            assert!(l.len() > 50, "a level must stand on its own: {l}");
            assert!(l.ends_with('.'), "{l}");
        }
    }

    #[test]
    fn the_top_level_normalizes_to_exactly_one_and_nothing_sorts_off_the_end() {
        // Divided by the gaps, not the levels — otherwise the most urgent
        // possible answer reads as 0.8 and no item is ever "first".
        assert_eq!(normalized(0.0), 0.0);
        assert_eq!(normalized(4.0), 1.0);
        assert_eq!(normalized(2.0), 0.5);
        // A provider answering outside its own range must not sort an item
        // past the ends.
        assert_eq!(normalized(9.0), 1.0);
        assert_eq!(normalized(-3.0), 0.0);
        assert_eq!(normalized(f64::NAN), 0.0);
        assert_eq!(normalized(f64::INFINITY), 0.0);
    }

    #[test]
    fn a_bucket_missing_one_judgment_keeps_its_whole_deterministic_order() {
        let keys: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let mut u = BTreeMap::new();
        u.insert("a".to_string(), 0.1);
        u.insert("b".to_string(), 0.9);
        // `c` has no usable judgment — so NOTHING is reordered, rather than
        // `c` being parked at one end of a partly-judged list.
        assert_eq!(reordered(&keys, &u), None);
        u.insert("c".to_string(), 0.5);
        assert_eq!(
            reordered(&keys, &u),
            Some(vec!["b".to_string(), "c".to_string(), "a".to_string()])
        );
        // And an empty bucket is not a reorder.
        assert_eq!(reordered(&[], &u), None);
    }

    #[test]
    fn equal_urgency_keeps_the_order_the_existing_policy_gave() {
        // THE PROPERTY THAT MAKES THIS SAFE TO SHIP: the judgment only breaks
        // ties. Scored identically, the items come back exactly as they went
        // in — so with a provider that answers everything the same, the brief
        // is byte-for-byte what it was.
        let keys: Vec<String> = ["first", "second", "third"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let u: BTreeMap<String, f64> = keys.iter().map(|k| (k.clone(), 0.5)).collect();
        assert_eq!(reordered(&keys, &u), Some(keys.clone()));
        // Reversing the input reverses the output, for the same reason.
        let rev: Vec<String> = keys.iter().rev().cloned().collect();
        assert_eq!(reordered(&rev, &u), Some(rev));
    }

    #[test]
    fn this_site_is_in_the_census_and_reads_its_floor_as_certainty() {
        // A Score has no "probability of yes" to lean on, so a lean floor over
        // one would read None and silently judge nothing.
        let def = sites::def_of(SITE).expect("focus-rank is in the census");
        assert_eq!(def.reads, sites::FLOOR_CERTAINTY);
        assert_eq!(def.primitive, "score");
        assert!(!def.default_on, "it measures until somebody turns it on");
    }
}

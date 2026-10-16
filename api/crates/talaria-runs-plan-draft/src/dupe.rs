// THE DEDUP PASS — "did we just draft a ticket the board already has?", asked
// of a decision model once per unflagged proposal, between the draft and the
// row the review reads.
//
// WHY IT IS A SECOND PASS at all, when the drafting model is already SHOWN the
// board's open tickets and can fill in `duplicateOf` itself: because it very
// often does not. The drafting turn is being asked to do six things at once —
// read a transcript, split it into tickets, size them, tag them, assign them,
// stack them under epics — and the one thing it is worst at is noticing that
// one of the six is something it was shown in a list further up the prompt. So
// the model's own flag is taken when it is there (`duplicate_of` already set
// means this pass asks nothing about that proposal), and everything it did not
// flag gets one narrow question with a short list of candidates and nothing
// else to think about. That is the shape gap-align already uses for the same
// question one surface over, and this is modelled on it deliberately.
//
// NOTHING HERE CAN FAIL A DRAFT. A board read that errors, a port that is off
// or unconfigured, an answer below the floor: every one of them means the
// batch goes to the row exactly as the model drafted it, which is what
// happened before this module existed. The pass can only ever untick.

use std::collections::BTreeSet;

use talaria_channel_plan::{BoardTicket, open_tickets};
use talaria_state::AppState;

use crate::StoredProposal;

/// The option id meaning "none of these". A named constant because it is
/// compared in two places — the baseline the ledger records and the answer the
/// act is skipped on — and a typo in either would turn every no-match into a
/// silent no-op that nothing ever looks wrong about.
const NO_MATCH: &str = "none";

/// How many open tickets a single question may offer. Six because the list has
/// to be short enough that the model reads all of it, and because the ranking
/// below is a token overlap — past the top handful it is offering coincidences.
const MAX_CANDIDATES: usize = 6;

/// How much of a proposal's description the question carries. Enough to tell
/// two similarly-titled tickets apart, short enough that six candidates and a
/// proposal still fit a small model's attention.
const DESCRIPTION_CHARS: usize = 400;

/// Words that say nothing about WHAT a ticket is. Dropped before the overlap,
/// because a pair of titles that share only "the" and "to" share nothing, and
/// an unfiltered Jaccard scores that pair above a pair that shares one rare
/// word — exactly backwards.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "in", "into", "is",
    "it", "its", "of", "on", "or", "our", "that", "the", "then", "this", "to", "up", "we", "when",
    "with",
];

/// Flag the proposals in this batch that re-propose an open ticket on the
/// board. In place, and a SLICE rather than a `&mut Vec`: nothing here adds or
/// drops a proposal — the batch that arrives is the batch the reviewer sees,
/// and the only thing this changes about one is its flag and its tick.
pub async fn flag_duplicates(state: &AppState, board_id: &str, proposals: &mut [StoredProposal]) {
    // One read for the whole batch, and a failure answers with no candidates
    // rather than with an error: see the header.
    let tickets = match open_tickets(&state.pg, board_id).await {
        Ok(tickets) => tickets,
        Err(e) => {
            tracing::warn!(
                board_id,
                error = %e,
                "plan-draft dedup: could not read the board's open tickets; drafting without it"
            );
            return;
        }
    };
    if tickets.is_empty() {
        return;
    }
    for p in unflagged(proposals) {
        let candidates = rank_candidates(&p.title, &tickets);
        if candidates.is_empty() {
            continue;
        }
        if let Some(id) = matched_ticket(state, p, &candidates).await {
            p.duplicate_of = Some(id);
            // The same untick `normalize` gives a proposal the model flagged
            // itself — one flag, one meaning, wherever it came from.
            p.include = false;
        }
    }
}

/// The proposals this pass asks about: the ones nothing has flagged yet.
///
/// A proposal the DRAFTING model already flagged is left exactly as it is, and
/// that is a judgment call worth naming: the model that flagged it had the
/// whole transcript and the whole open-ticket list in front of it, while this
/// pass has one title and six candidates. Re-asking could only overwrite the
/// better-informed answer with the worse one, and the untick is already there
/// either way.
fn unflagged(proposals: &mut [StoredProposal]) -> impl Iterator<Item = &mut StoredProposal> {
    proposals.iter_mut().filter(|p| p.duplicate_of.is_none())
}

/// The open tickets worth asking about for this title: the best few by token
/// overlap, and only ones that overlap at all.
///
/// A CHEAP RANKING IN FRONT OF AN EXPENSIVE QUESTION. The overlap is not the
/// answer — "Fix the billing webhook" and "Document the billing webhook" score
/// identically against a proposal about billing webhooks, and only a model can
/// say which of them is the same work. What the overlap is for is the LIST: a
/// board with sixty open tickets cannot be a sixty-option question, and a
/// question whose options are sixty tickets in no particular order is one the
/// model answers by picking something plausible.
fn rank_candidates<'a>(title: &str, tickets: &'a [BoardTicket]) -> Vec<&'a BoardTicket> {
    let mut scored: Vec<(f32, &BoardTicket)> = tickets
        .iter()
        .map(|t| (overlap(title, &t.title), t))
        .filter(|(score, _)| *score > 0.0)
        .collect();
    // A stable sort, so tickets that score the same keep the order the read
    // gave them — newest first. Descending by score, and `total_cmp` rather
    // than `partial_cmp().unwrap()`: the scores are finite by construction,
    // and a panic in a pass that is not allowed to fail a draft would be a
    // strange way to find out otherwise.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, t)| t)
        .collect()
}

/// Token-set Jaccard over two titles, lowercased, stopwords dropped: the share
/// of the words either title uses that BOTH of them use.
///
/// Jaccard rather than a count of shared words, because a count rewards length
/// — a ten-word title shares more words with everything — and the question
/// being ranked for is "same work", which a long title and a short one can
/// perfectly well be. 0.0 whenever either side has no words left after the
/// stopwords go, which is what makes "the" a non-match instead of a perfect
/// one.
fn overlap(title_a: &str, title_b: &str) -> f32 {
    let a = tokens(title_a);
    let b = tokens(title_b);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let shared = a.intersection(&b).count();
    if shared == 0 {
        return 0.0;
    }
    let union = a.len() + b.len() - shared;
    shared as f32 / union as f32
}

/// A title's content words, as a set. Split on anything that is not
/// alphanumeric, so `webhook:`, `(billing)` and `invoices,` are the words they
/// are; a SET rather than a list because "billing billing billing" is one
/// thing to be about.
fn tokens(title: &str) -> BTreeSet<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// The one question, and the ledger row behind it: which of these open tickets
/// is this proposal, or none of them.
///
/// `None` for every reason the port can have nothing — off, unconfigured,
/// unable to serve a Choice, below the operator's floor, or a confident "none
/// of them" — so an install that never configures a decision model drafts
/// exactly as it always has.
async fn matched_ticket(
    state: &AppState,
    proposal: &StoredProposal,
    candidates: &[&BoardTicket],
) -> Option<String> {
    // NO_MATCH FIRST, and it is the question's whole safety: an options list
    // of six tickets with no way out is a list the model picks from, and the
    // honest answer to most proposals is that none of them is it.
    let mut options = vec![talaria_decide::Opt::new(
        NO_MATCH,
        "none of them — this proposal is work the board does not already have open, and should be \
         created as a new ticket",
    )];
    for (i, c) in candidates.iter().enumerate() {
        options.push(talaria_decide::Opt::new(i.to_string(), label_of(c)));
    }

    let description: String = proposal
        .description
        .chars()
        .take(DESCRIPTION_CHARS)
        .collect();
    let ask = talaria_decide::Ask::new(serde_json::json!({
        "proposal": { "title": proposal.title, "description": description },
    }))
    .q(
        "same",
        talaria_decide::Question::choice(
            "Which of the listed open tickets is THE SAME WORK as `proposal` — the same thing to \
             build or fix, not merely the same area or system?",
            options,
        ),
    );

    let http = talaria_retrieval_http::real_http();
    let answers = talaria_decide::decide(state, &http, &ask).await?;
    let j = answers.get("same")?;
    // Every judgment is recorded, the no-matches and the below-floor ones
    // included: an agreement rate measured over only the flags is not one.
    //
    // THE BASELINE IS NO_MATCH because that is what the code did before this
    // module — the drafting model did not flag this proposal, so the existing
    // behaviour is "create it". The ledger's agreement column therefore reads
    // as "how often does the port leave the batch alone", which is the number
    // an operator deciding whether to trust the untick actually wants.
    talaria_decide::shadow::record(
        &state.pg,
        &talaria_decide::shadow::Compare {
            site: "ticket-dupe",
            // The draft id would be the row to go and read, and this pass does
            // not have one: it is handed a batch, not the row the batch is
            // going into. A hash of the title is the next most useful thing —
            // stable across redrafts of the same conversation, so an operator
            // reading the ledger can see the same proposal asked about twice.
            subject_ref: Some(title_ref(&proposal.title)),
            baseline: NO_MATCH.to_string(),
            agrees: |a, baseline| a.chosen() == Some(baseline),
        },
        Some(j),
    )
    .await;

    // The switch and the floor are the operator's, read per call through the
    // site census. `None` means "do what you did before", which here is leave
    // the proposal ticked and let the reviewer read the board themselves.
    let j = talaria_decide::acted(&state.pg, "ticket-dupe", Some(j.clone())).await?;
    let chosen = j.answer.chosen()?;
    if chosen == NO_MATCH {
        return None;
    }
    let i: usize = chosen.parse().ok()?;
    candidates.get(i).map(|c| c.id.clone())
}

/// A candidate as the model reads it: the ticket number a human would quote,
/// then the title. The ID is deliberately NOT here — the answer is an option
/// id, so a uuid in the text is a second identifier for the model to confuse
/// the first one with.
fn label_of(t: &BoardTicket) -> String {
    match t.ticket_no {
        Some(no) => format!("#{no} {}", t.title.trim()),
        None => t.title.trim().to_string(),
    }
}

/// A stable, short handle for a proposal title — FNV-1a, hex. Written out
/// rather than taken from `DefaultHasher` because the ledger is read across
/// restarts and across releases, and `DefaultHasher`'s output is explicitly
/// not promised to be the same twice: two rows about one title would look like
/// two titles.
fn title_ref(title: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in title.trim().to_lowercase().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("proposal:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize;
    use talaria_harness_defs::defs::channel_plan::{Priority, TicketProposal};

    fn ticket(id: &str, no: i32, title: &str) -> BoardTicket {
        BoardTicket {
            id: id.into(),
            ticket_no: Some(no),
            status: "inbox".into(),
            title: title.into(),
        }
    }

    fn proposal(title: &str, duplicate_of: Option<&str>) -> StoredProposal {
        normalize(TicketProposal {
            title: title.into(),
            description: "the work".into(),
            priority: Priority::Medium,
            effort: None,
            depends_on: Vec::new(),
            tags: Vec::new(),
            parent: None,
            assignees: Vec::new(),
            duplicate_of: duplicate_of.map(str::to_string),
        })
    }

    #[test]
    fn overlap_scores_shared_content_words_and_ignores_the_rest() {
        // The same four content words in a different order is the same thing
        // said twice: the set has no order, so neither does the score.
        assert_eq!(
            overlap(
                "Fix the billing webhook retry",
                "Retry fix of webhook billing"
            ),
            1.0
        );
        // Stopwords carry no information, so a pair that shares only stopwords
        // shares nothing — the one case an unfiltered Jaccard gets loudly
        // wrong, scoring "The work to be done" against "The thing to be fixed"
        // above a genuine one-rare-word match.
        assert_eq!(overlap("The work to be done", "The thing to be fixed"), 0.0);
        // And a title with nothing BUT stopwords has no words to share.
        assert_eq!(overlap("the and of", "Fix the billing webhook"), 0.0);
        assert_eq!(overlap("", "Fix the billing webhook"), 0.0);
        // Case and punctuation are not content either.
        assert_eq!(overlap("BILLING, WEBHOOK!", "billing webhook"), 1.0);
        // Partial, and the arithmetic spelled out because the denominator is
        // the UNION: {fix, billing, webhook} against {retry, billing,
        // invoices} shares one word out of the five distinct words the pair
        // uses between them.
        let partial = overlap("Fix the billing webhook", "Retry the billing invoices");
        assert!(
            (partial - 1.0 / 5.0).abs() < 1e-6,
            "one shared word of five, got {partial}"
        );
        // Length alone does not earn a score: a long title that happens to
        // contain a short one must not outrank the short one's exact twin.
        assert!(
            overlap("Billing webhook", "Billing webhook")
                > overlap(
                    "Billing webhook",
                    "Billing webhook retries, invoices, dunning and receipts"
                )
        );
    }

    #[test]
    fn the_candidate_list_is_the_best_few_that_overlap_at_all() {
        let tickets = vec![
            ticket("t-1", 1, "Rewrite the onboarding emails"),
            ticket("t-2", 2, "Fix the billing webhook"),
            ticket("t-3", 3, "Billing invoices retry"),
            ticket("t-4", 4, "Audit the webhook signing secret"),
        ];
        let ranked = rank_candidates("Fix the billing webhook retries", &tickets);
        // Ordered best first, and the ticket about onboarding emails — which
        // shares no content word — is not offered at all. A zero-overlap
        // option is an invitation to pick a coincidence.
        assert_eq!(
            ranked.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["t-2", "t-3", "t-4"]
        );

        // Nothing overlaps: no question gets asked, which is the cut that
        // keeps a board of unrelated work from costing a call per proposal.
        assert!(rank_candidates("Upgrade the Postgres major version", &tickets).is_empty());

        // And the list is capped: a board where everything mentions billing
        // still asks about six tickets, not sixty.
        let many: Vec<BoardTicket> = (0..20)
            .map(|i| ticket(&format!("t-{i}"), i, "Billing webhook work"))
            .collect();
        assert_eq!(
            rank_candidates("Billing webhook", &many).len(),
            MAX_CANDIDATES
        );
        // Ties keep the read's order, which is newest first.
        assert_eq!(
            rank_candidates("Billing webhook", &many)
                .first()
                .map(|t| t.id.as_str()),
            Some("t-0")
        );
    }

    #[test]
    fn a_proposal_the_model_already_flagged_is_left_alone() {
        let mut batch = vec![
            proposal("Fix the billing webhook", Some("t-1")),
            proposal("Retry failed invoices", None),
            proposal("Document the webhook contract", None),
        ];
        // The walk asks about the two nothing has flagged, and does not spend
        // a decision call re-litigating the one the drafting model answered
        // with the whole transcript in front of it.
        let asked: Vec<String> = unflagged(&mut batch).map(|p| p.title.clone()).collect();
        assert_eq!(
            asked,
            vec!["Retry failed invoices", "Document the webhook contract"]
        );
        // And the flagged proposal still carries the flag and the untick it
        // arrived with — this pass only ever adds flags.
        assert_eq!(batch[0].duplicate_of.as_deref(), Some("t-1"));
        assert!(!batch[0].include);
        assert!(batch[1].include && batch[2].include);
    }

    #[test]
    fn a_candidates_label_names_the_ticket_a_human_would_quote() {
        assert_eq!(
            label_of(&ticket("t-1", 12, "  Fix the billing webhook ")),
            "#12 Fix the billing webhook"
        );
        // A ticket with no number yet is its title, with no empty `#`.
        let mut unnumbered = ticket("t-2", 0, "Retry failed invoices");
        unnumbered.ticket_no = None;
        assert_eq!(label_of(&unnumbered), "Retry failed invoices");
    }

    #[test]
    fn a_titles_ledger_handle_is_the_same_handle_next_time() {
        // Stable across calls and insensitive to the edges a redraft changes;
        // different titles are different rows.
        assert_eq!(
            title_ref("Fix the billing webhook"),
            title_ref("  fix the BILLING webhook  ")
        );
        assert_ne!(
            title_ref("Fix the billing webhook"),
            title_ref("Fix the billing webhooks")
        );
        assert!(title_ref("anything").starts_with("proposal:"));
    }
}

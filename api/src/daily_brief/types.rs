// The daily brief's wire shapes and the fold — ports of
// ui/src/server/daily-brief-types.ts and ui/src/server/daily-brief-fold.ts.
//
// The fold is PURE, and in its own module for the same two reasons as the TS
// file: it is the one piece of the brief with no database, no clock and no
// model in it, which makes it the piece worth testing directly — and the sweep
// computes its diff against the FOLD rather than against a snapshot it wrote
// earlier, so a bug here is a bug in change detection, not just in rendering.
// The artifact mirror renders the same fold, which is the other reason it
// cannot live in the core module.

use serde_json::Value;

/// The five places a line can land in the document. Ordered as the document
/// reads, and that order is load-bearing — the fold sorts sections by this
/// array, so a section added in the middle moves the document, not just a
/// switch statement somewhere.
pub const BRIEF_SECTIONS: [&str; 4] = ["action", "schedule", "comms", "highlights"];

/// The kinds that CLOSE a line.
///
/// Three of them, because who closed it is worth keeping: the source stopped
/// needing you (`resolved`), you did the thing (`checked`), or you decided it
/// did not need doing (`dismissed`). All three sink the line and strike it
/// through; none of them removes it, because nothing here removes anything.
///
/// A MATCH RATHER THAN A COMPARISON, because the fold, the sweep and the render
/// each have to ask "is this line closed" and the version of this feature with
/// `kind == "resolved"` written out three times is the version where adding a
/// fourth kind silently un-closes lines in whichever place got missed.
pub fn is_terminal(kind: &str) -> bool {
    matches!(kind, "resolved" | "checked" | "dismissed")
}

/// What an appended row IS. These are not statuses on a mutable row — they are
/// the verbs of an append-only log, and each one is a row that exists forever:
/// lede the assistant's opening read (seq 1, exactly once per brief); item a
/// source appearing for the first time today; change the same source,
/// materially different — supersedes the last; resolved the source stopped
/// needing the owner — supersedes the last; note the assistant narrating a
/// batch of appends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefEntry {
    pub id: String,
    pub seq: i64,
    /// Which append wrote this row. Entries sharing a batch were learned in the
    /// same sweep and are shown as one moment in the timeline. Null on rows
    /// written before the column existed.
    pub batch: Option<String>,
    pub kind: String,
    pub section: String,
    pub source_key: Option<String>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    /// Where in Talaria this line points. THE POINT OF THE SURFACE — a brief
    /// that names a blocked ticket and cannot open it is a newsletter.
    pub source_href: Option<String>,
    pub fingerprint: Option<String>,
    pub supersedes: Option<String>,
    pub priority: Option<String>,
    pub status_label: Option<String>,
    /// `{"label": …, "tone": …}` — stored jsonb, read as-is.
    pub badge: Option<Value>,
    pub title: String,
    pub body: String,
    /// `[{label, text}]` — stored jsonb, read as-is.
    pub evidence: Value,
    pub created_at: String,
}

/// One row on its way into the log. No id and no seq — `append_entries` owns
/// both, which is what keeps seq allocation atomic under overlap.
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub kind: String,
    pub section: String,
    pub source_key: Option<String>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub source_href: Option<String>,
    pub fingerprint: Option<String>,
    pub supersedes: Option<String>,
    pub priority: Option<String>,
    pub status_label: Option<String>,
    pub badge: Option<Value>,
    pub title: String,
    pub body: String,
    pub evidence: Value,
}

impl NewEntry {
    /// The TS object literals build entries with `evidence` defaulted to `[]`
    /// and `badge` to whatever the source computed (or null) — this is that
    /// constructor, for the call sites that have neither.
    pub fn narrative(
        kind: &str,
        section: &str,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.to_string(),
            section: section.to_string(),
            source_key: None,
            source_type: None,
            source_id: None,
            source_href: None,
            fingerprint: None,
            supersedes: None,
            priority: None,
            status_label: None,
            badge: None,
            title: title.into(),
            body: body.into(),
            evidence: Value::Array(Vec::new()),
        }
    }
}

/// A source folded to its CURRENT state — the newest entry for a key, plus the
/// trail of every earlier entry for it.
///
/// `history` is why the fold exists rather than a `select distinct on`. The
/// document shows where a thing stands; the thread underneath shows how it got
/// there, and both are read off the same log.
#[derive(Debug, Clone)]
pub struct BriefLine {
    pub key: String,
    pub section: String,
    /// The newest entry for this key — what the row renders as right now.
    pub current: BriefEntry,
    /// Every entry for this key, oldest first, `current` last.
    pub history: Vec<BriefEntry>,
    /// Superseded by a terminal entry: struck through, kept on the page.
    pub resolved: bool,
    /// Changed since the reader's `read_seq`. Drives the "new" affordance.
    pub unseen: bool,
}

/// An append event, as the day's timeline shows it: one sweep's worth of
/// entries under the assistant's note about them.
#[derive(Debug, Clone)]
pub struct BriefUpdate {
    pub seq: i64,
    pub at: String,
    /// The assistant's line about this batch, when it wrote one.
    pub note: Option<String>,
    pub entries: Vec<BriefEntry>,
}

/// Replay the log into the document.
///
/// This is the only place the current state of a brief is computed, and it is a
/// pure function of the rows. That is what lets the sweep diff against the FOLD
/// rather than against a snapshot it wrote earlier: a sweep that died half-way
/// leaves a shorter log, never a wrong one, and the next sweep sees exactly
/// what is missing.
pub fn fold_entries(mut entries: Vec<BriefEntry>, read_seq: i64) -> Folded {
    // Postgres gives no ordering guarantee we have not asked for, and a fold
    // that trusted insertion order would report a resolved item as open the
    // first time a query came back the other way round. (Rust's sort is stable
    // like the TS spread-sort, so equal seqs keep arrival order.)
    entries.sort_by_key(|a| a.seq);

    // TS holds lines in a Map keyed by source key, whose iteration order is
    // first-insertion — i.e. by the first seq each key was seen at. A Vec of
    // pairs keeps that order without an order-preserving map type; the update
    // goes by position because a closure returning `&mut` into the captured
    // vec cannot escape its own body.
    let mut lines: Vec<(String, BriefLine)> = Vec::new();

    for entry in &entries {
        let Some(source_key) = entry.source_key.clone() else {
            continue;
        };
        if let Some(i) = lines.iter().position(|(k, _)| *k == source_key) {
            let (_, existing) = &mut lines[i];
            existing.unseen = existing.unseen || entry.seq > read_seq;
            existing.resolved = is_terminal(&entry.kind);
            existing.current = entry.clone();
            existing.history.push(entry.clone());
            continue;
        }
        lines.push((
            source_key.clone(),
            BriefLine {
                key: source_key,
                section: entry.section.clone(),
                current: entry.clone(),
                history: vec![entry.clone()],
                resolved: is_terminal(&entry.kind),
                unseen: entry.seq > read_seq,
            },
        ));
    }

    // The day, batched. Entries written by one append share a `batch`, and
    // grouping on it is what turns a flat log into "at 11:04, three things
    // moved" — the shape a person actually reads a day in.
    //
    // The fallback is for rows written before `batch` existed: truncating
    // `created_at` to the second was the original grouping, and it is right
    // except when two appends land in the same second, which is exactly why the
    // column was added. Keeping it means an existing brief still renders as a
    // timeline rather than as one undifferentiated blob.
    let mut batches: Vec<(String, Vec<&BriefEntry>)> = Vec::new();
    for entry in &entries {
        if entry.kind == "lede" {
            continue;
        }
        let bucket = entry.batch.clone().unwrap_or_else(|| {
            format!(
                "t:{}",
                entry.created_at.chars().take(19).collect::<String>()
            )
        });
        match batches.iter_mut().find(|(k, _)| *k == bucket) {
            Some((_, group)) => group.push(entry),
            None => batches.push((bucket, vec![entry])),
        }
    }

    let mut updates: Vec<BriefUpdate> = batches
        .into_iter()
        .map(|(_, group)| {
            // `note?.body || note?.title || null`: the body, else the title,
            // else nothing. An empty string falls through like a missing one.
            let note = group
                .iter()
                .find(|e| e.kind == "note" && e.source_key.is_none());
            let note_id = note.map(|n| n.id.clone());
            let note_text = note.map(|n| {
                if n.body.is_empty() {
                    n.title.clone()
                } else {
                    n.body.clone()
                }
            });
            BriefUpdate {
                seq: group.iter().map(|e| e.seq).max().unwrap_or(0),
                at: group[0].created_at.clone(),
                note: note_text,
                entries: group
                    .iter()
                    // The note rides the batch as its heading, not as a row.
                    .filter(|e| Some(&e.id) != note_id.as_ref())
                    .map(|e| (*e).clone())
                    .collect(),
            }
        })
        .filter(|u| !u.entries.is_empty() || u.note.is_some())
        .collect();
    // Newest first: a person checking back at 14:00 wants the last thing that
    // happened at the top. (The mirrored artifact reverses this deliberately —
    // a shared document is read as a narrative.)
    updates.sort_by_key(|u| std::cmp::Reverse(u.seq));

    Folded {
        lines: lines.into_iter().map(|(_, l)| l).collect(),
        updates,
    }
}

pub struct Folded {
    pub lines: Vec<BriefLine>,
    pub updates: Vec<BriefUpdate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // THE FOLD IS THE CHANGE DETECTOR, not just a renderer. The sweep diffs the
    // live sources against this function's output, so every bug here is a bug in
    // what gets appended — a line the fold loses is a line the next sweep re-adds
    // as new, and a resolution the fold misses is a line that never comes back.
    fn seq_cell() -> i64 {
        use std::sync::atomic::{AtomicI64, Ordering};
        static SEQ: AtomicI64 = AtomicI64::new(0);
        SEQ.fetch_add(1, Ordering::SeqCst)
    }

    fn entry(over: impl FnOnce(&mut BriefEntry)) -> BriefEntry {
        let seq = seq_cell();
        let mut e = BriefEntry {
            id: format!("e{seq}"),
            seq,
            batch: None,
            kind: "item".into(),
            section: "action".into(),
            source_key: None,
            source_type: None,
            source_id: None,
            source_href: None,
            fingerprint: None,
            supersedes: None,
            priority: Some("p1".into()),
            status_label: None,
            badge: None,
            title: "A thing".into(),
            body: String::new(),
            evidence: Value::Array(Vec::new()),
            created_at: "2026-08-17T07:00:00.000Z".into(),
        };
        over(&mut e);
        e
    }

    fn keys(folded: &Folded) -> Vec<&str> {
        folded.lines.iter().map(|l| l.key.as_str()).collect()
    }

    #[test]
    fn collapses_a_key_to_its_newest_entry_and_keeps_the_whole_trail() {
        let first = entry(|e| {
            e.source_key = Some("task:1".into());
            e.title = "Unblock Ledger migration".into();
            e.fingerprint = Some("a".into());
        });
        let second = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "change".into();
            e.fingerprint = Some("b".into());
            e.supersedes = Some(first.id.clone());
        });

        let folded = fold_entries(vec![first.clone(), second.clone()], 0);

        assert_eq!(folded.lines.len(), 1);
        assert_eq!(folded.lines[0].current.id, second.id);
        // THE TRAIL IS THE PRODUCT, not debug data: "3 updates today" on a row
        // is the only thing that tells a reader this line has a history worth
        // opening.
        assert_eq!(
            folded.lines[0]
                .history
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            vec![first.id.as_str(), second.id.as_str()]
        );
    }

    #[test]
    fn marks_a_line_resolved_without_dropping_it_from_the_document() {
        let first = entry(|e| {
            e.source_key = Some("task:1".into());
            e.fingerprint = Some("a".into());
        });
        let done = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "resolved".into();
            e.supersedes = Some(first.id.clone());
        });

        let folded = fold_entries(vec![first, done], 0);

        // A RESOLVED LINE STAYS. This is the append-only contract's whole
        // visible consequence: what somebody read at 08:00 is still findable at
        // 18:00. A fold that filtered these out would make the document lie by
        // omission and would also make the next sweep re-append the item as new.
        assert_eq!(folded.lines.len(), 1);
        assert!(folded.lines[0].resolved);
    }

    #[test]
    fn un_resolves_a_line_that_comes_back_rather_than_starting_a_second_one() {
        let first = entry(|e| {
            e.source_key = Some("task:1".into());
            e.fingerprint = Some("a".into());
        });
        let done = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "resolved".into();
            e.supersedes = Some(first.id.clone());
        });
        let again = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "change".into();
            e.fingerprint = Some("c".into());
            e.supersedes = Some(done.id.clone());
        });

        let folded = fold_entries(vec![first, done, again], 0);

        assert_eq!(folded.lines.len(), 1);
        assert!(!folded.lines[0].resolved);
        assert_eq!(folded.lines[0].history.len(), 3);
    }

    #[test]
    fn reads_the_log_in_seq_order_regardless_of_arrival_order() {
        let first = entry(|e| {
            e.source_key = Some("task:1".into());
            e.fingerprint = Some("a".into());
        });
        let done = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "resolved".into();
            e.supersedes = Some(first.id.clone());
        });

        let folded = fold_entries(vec![done.clone(), first], 0);

        assert_eq!(folded.lines[0].current.id, done.id);
        assert!(folded.lines[0].resolved);
    }

    #[test]
    fn flags_a_line_unseen_when_anything_landed_after_the_read_cursor() {
        let mk = |seq: i64, kind: &str| {
            entry(|e| {
                e.seq = seq;
                e.source_key = Some("task:1".into());
                e.kind = kind.into();
                e.fingerprint = Some(if seq == 1 { "a" } else { "b" }.into());
            })
        };
        let first = mk(1, "item");
        let changed = mk(5, "change");

        assert!(!fold_entries(vec![first.clone(), changed.clone()], 5).lines[0].unseen);
        assert!(fold_entries(vec![first.clone(), changed.clone()], 4).lines[0].unseen);
        // Seen the change but not the original is not a state that can occur,
        // and the OR is what keeps a line the reader has never seen marked new.
        assert!(fold_entries(vec![first, changed], 0).lines[0].unseen);
    }

    #[test]
    fn ignores_narrative_entries_and_heads_a_batch_with_its_note() {
        let lede = entry(|e| {
            e.kind = "lede".into();
            e.title = "Daily brief".into();
            e.body = "Two things need you.".into();
        });
        let note = entry(|e| {
            e.kind = "note".into();
            e.body = "The webhook review was signed off.".into();
            e.batch = Some("b1".into());
        });
        let item = entry(|e| {
            e.source_key = Some("task:9".into());
            e.title = "Reply to Dana".into();
            e.batch = Some("b1".into());
        });

        let folded = fold_entries(vec![lede, note, item.clone()], 0);

        // Neither the lede nor the note has a source key, so neither becomes a
        // row in the document — they are the voice around the rows.
        assert_eq!(keys(&folded), vec!["task:9"]);
        assert_eq!(folded.updates.len(), 1);
        assert_eq!(
            folded.updates[0].note.as_deref(),
            Some("The webhook review was signed off.")
        );
        assert_eq!(
            folded.updates[0]
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec![item.id.as_str()]
        );
    }

    #[test]
    fn excludes_the_lede_from_the_timeline_entirely() {
        let lede = entry(|e| {
            e.kind = "lede".into();
            e.body = "Two things need you.".into();
        });
        // The opening read is the head of the DOCUMENT, not the first thing
        // that happened today.
        assert!(fold_entries(vec![lede], 0).updates.is_empty());
    }

    #[test]
    fn groups_by_the_append_and_lists_batches_newest_first() {
        let morning = entry(|e| {
            e.seq = 2;
            e.source_key = Some("task:1".into());
            e.batch = Some("b1".into());
        });
        let noon_a = entry(|e| {
            e.seq = 3;
            e.source_key = Some("task:2".into());
            e.batch = Some("b2".into());
        });
        let noon_b = entry(|e| {
            e.seq = 4;
            e.source_key = Some("task:3".into());
            e.batch = Some("b2".into());
        });

        let folded = fold_entries(vec![morning, noon_a.clone(), noon_b.clone()], 0);

        assert_eq!(folded.updates.len(), 2);
        assert_eq!(
            folded.updates[0]
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec![noon_a.id.as_str(), noon_b.id.as_str()]
        );
        assert_eq!(folded.updates[1].entries.len(), 1);
    }

    #[test]
    fn keeps_two_appends_in_the_same_second_apart() {
        // THE WHOLE REASON `batch` IS STORED. Grouping used to truncate
        // `created_at` to the second, which is right until a realtime nudge and
        // a scheduler tick reach the sweep together.
        let first = entry(|e| {
            e.seq = 2;
            e.source_key = Some("task:1".into());
            e.batch = Some("b1".into());
        });
        let second = entry(|e| {
            e.seq = 3;
            e.source_key = Some("task:2".into());
            e.batch = Some("b2".into());
        });
        assert_eq!(fold_entries(vec![first, second], 0).updates.len(), 2);
    }

    #[test]
    fn falls_back_to_the_timestamp_for_rows_before_the_batch_column() {
        let morning = entry(|e| {
            e.seq = 2;
            e.source_key = Some("task:1".into());
            e.created_at = "2026-08-17T09:00:00.000Z".into();
        });
        let noon = entry(|e| {
            e.seq = 3;
            e.source_key = Some("task:2".into());
            e.created_at = "2026-08-17T12:00:00.000Z".into();
        });
        assert_eq!(fold_entries(vec![morning, noon], 0).updates.len(), 2);
    }

    // THE OWNER CAN CLOSE A LINE TOO. `resolved` is the source saying so;
    // `checked` and `dismissed` are the person saying so, and all three have to
    // read as closed everywhere or a line struck through in one place is live
    // in another.
    #[test]
    fn treats_an_owner_closed_entry_as_closing_the_line() {
        for kind in ["checked", "dismissed"] {
            let first = entry(|e| {
                e.source_key = Some("task:1".into());
                e.fingerprint = Some("a".into());
            });
            let closed = entry(|e| {
                e.source_key = Some("task:1".into());
                e.kind = kind.into();
                e.supersedes = Some(first.id.clone());
                e.fingerprint = Some("a".into());
            });
            let folded = fold_entries(vec![first, closed], 0);
            assert_eq!(folded.lines.len(), 1, "{kind}");
            assert!(folded.lines[0].resolved, "{kind}");
            // Still on the page, like every other closed line — nothing here
            // removes anything, and a dismissal you cannot find is a deletion.
            assert_eq!(folded.lines[0].history.len(), 2);
        }
    }

    #[test]
    fn carries_the_source_fingerprint_onto_the_closing_entry() {
        // THE FIELD THAT MAKES A DISMISSAL STICK. The sweep skips a line whose
        // source fingerprint has not moved; if the dismissal dropped it, every
        // sweep would see a mismatch and reopen what the owner just closed.
        let first = entry(|e| {
            e.source_key = Some("task:1".into());
            e.fingerprint = Some("a".into());
        });
        let dismissed = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "dismissed".into();
            e.supersedes = Some(first.id.clone());
            e.fingerprint = Some("a".into());
        });
        let folded = fold_entries(vec![first, dismissed], 0);
        assert_eq!(folded.lines[0].current.fingerprint.as_deref(), Some("a"));
    }

    #[test]
    fn reopens_on_a_later_change_because_the_source_really_did_move() {
        // Dismissing a conversation must not become a mute button on the person
        // in it: if they say something else, that is new information.
        let first = entry(|e| {
            e.source_key = Some("channel:c1".into());
            e.fingerprint = Some("a".into());
        });
        let dismissed = entry(|e| {
            e.source_key = Some("channel:c1".into());
            e.kind = "dismissed".into();
            e.supersedes = Some(first.id.clone());
            e.fingerprint = Some("a".into());
        });
        let moved = entry(|e| {
            e.source_key = Some("channel:c1".into());
            e.kind = "change".into();
            e.supersedes = Some(dismissed.id.clone());
            e.fingerprint = Some("b".into());
        });
        let folded = fold_entries(vec![first, dismissed, moved], 0);
        assert!(!folded.lines[0].resolved);
        assert_eq!(folded.lines[0].history.len(), 3);
    }

    #[test]
    fn lets_a_restore_reopen_a_checked_line() {
        // `restore` appends a `change` carrying the last live state — the way
        // to take something back in an append-only log is to say the next thing.
        let first = entry(|e| {
            e.source_key = Some("task:1".into());
            e.fingerprint = Some("a".into());
            e.status_label = Some("FAILED".into());
        });
        let checked = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "checked".into();
            e.supersedes = Some(first.id.clone());
            e.fingerprint = Some("a".into());
        });
        let restored = entry(|e| {
            e.source_key = Some("task:1".into());
            e.kind = "change".into();
            e.supersedes = Some(checked.id.clone());
            e.fingerprint = Some("a".into());
            e.status_label = Some("FAILED".into());
        });
        let folded = fold_entries(vec![first, checked, restored], 0);
        assert!(!folded.lines[0].resolved);
        assert_eq!(
            folded.lines[0].current.status_label.as_deref(),
            Some("FAILED")
        );
    }
}

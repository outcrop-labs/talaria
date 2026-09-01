// The read-side assembly — port of daily-brief.ts's `view`/`documentView`.
// The brief's BriefView is a fixed shape, so it gets typed structs in TS
// declaration order rather than per-site `json!`.
//
// THE DOCUMENT IS THE LOG FOLDED. `view` takes the row, the entries, the
// assistant and the live comms state and answers the whole surface: sections
// in BRIEF_SECTIONS order, each line sorted unresolved-first then by priority
// then by where the log first put it, updates newest-batch-first, and the
// comms controls keyed onto the lines by source key.

use serde::Serialize;
use serde_json::Value;

use super::comms::CommsLine;
use super::focus::{as_iso, nullable_iso};
use super::types::{BRIEF_SECTIONS, BriefEntry, BriefLine, BriefUpdate, Folded, fold_entries};
use super::{BriefAssistant, BriefRow};

/// The parts of a conversation line that are LIVE rather than historical —
/// read fresh on every request, never from the log. The `draft` is a Value
/// because the two states spell different literals: a fresh draft projects to
/// `{id, content, stale: false}` (three keys) while a stale one passes the
/// whole `{id, content, stale, createdAt}` — the createdAt is what tells the
/// surface how long the decision has been waiting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommsStateWire {
    pub source_key: String,
    pub channel_id: String,
    /// The assistant may answer here without asking.
    pub delegated: bool,
    /// A reply waiting for the owner's decision.
    pub draft: Option<Value>,
}

/// commsLines → the wire control, exactly as documentView maps it: a live
/// draft projects to its three-key literal, a stale one rides whole.
pub fn comms_state(line: &CommsLine) -> CommsStateWire {
    let draft = line.draft.as_ref().map(|d| {
        if d.stale {
            serde_json::json!({ "id": d.id, "content": d.content, "stale": true, "createdAt": d.created_at })
        } else {
            serde_json::json!({ "id": d.id, "content": d.content, "stale": false })
        }
    });
    CommsStateWire {
        source_key: line.key.clone(),
        channel_id: line.channel_id.clone(),
        delegated: line.delegated,
        draft,
    }
}

/// One document section — the literal BRIEF_SECTIONS.map spells, in that key
/// order. `section` borrows from the const array, so it is `'static`.
#[derive(Serialize)]
pub struct SectionWire {
    section: &'static str,
    lines: Vec<BriefLine>,
}

/// The whole surface, folded (BriefView) — key order is the TS literal's.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefViewWire {
    pub id: String,
    /// Local calendar date the brief belongs to (YYYY-MM-DD).
    pub date: String,
    pub zone: String,
    pub opened_at: String,
    /// The assistant's opening read. Written once, never rewritten — a brief
    /// whose lede changed at noon would be a different document.
    pub lede: String,
    pub agent: BriefAssistant,
    pub artifact_id: Option<String>,
    /// Current state of every source, grouped by section, in document order.
    pub sections: Vec<SectionWire>,
    /// The day as it accumulated, newest batch first.
    pub updates: Vec<BriefUpdate>,
    /// Live conversation state, keyed onto `sections` lines by `sourceKey`.
    pub comms: Vec<CommsStateWire>,
    pub last_seq: i64,
    pub read_seq: i64,
    /// Appended since the reader last looked.
    pub unseen_count: usize,
    pub last_swept_at: Option<String>,
}

/// The rank behind "priority first, then the order the log put them in".
/// Unknown priorities sink with the `ok` tier, as the TS `?? 3` does.
fn priority_rank(priority: Option<&str>) -> i64 {
    match priority {
        Some("p0") => 0,
        Some("p1") => 1,
        Some("p2") => 2,
        _ => 3,
    }
}

/// view() — the one assembly behind both the today row and the recent-brief
/// fallback. A resolved line sinks but is NEVER dropped: the thing a person
/// read this morning has to still be findable this afternoon, struck through,
/// which is the whole argument for an append-only document over a queue.
pub fn view(
    row: &BriefRow,
    entries: &[BriefEntry],
    agent: &BriefAssistant,
    comms: Vec<CommsStateWire>,
) -> BriefViewWire {
    let Folded { lines, updates } = fold_entries(entries.to_vec(), row.read_seq);
    let lede = entries.iter().find(|e| e.kind == "lede");

    let sections = BRIEF_SECTIONS
        .iter()
        .map(|&section| {
            let mut group: Vec<BriefLine> = lines
                .iter()
                .filter(|l| l.section == section)
                .cloned()
                .collect();
            group.sort_by(|a, b| {
                // Unresolved first (false < true), then priority, then the seq
                // the log first put the line at. Rust's sort is stable, so
                // ties keep fold order — first-insertion, the order TS's Map
                // held the lines in.
                a.resolved
                    .cmp(&b.resolved)
                    .then_with(|| {
                        priority_rank(a.current.priority.as_deref())
                            .cmp(&priority_rank(b.current.priority.as_deref()))
                    })
                    .then_with(|| a.history[0].seq.cmp(&b.history[0].seq))
            });
            SectionWire {
                section,
                lines: group,
            }
        })
        .filter(|s| !s.lines.is_empty())
        .collect();

    BriefViewWire {
        id: row.id.clone(),
        date: row.brief_date.clone(),
        zone: row.zone.clone(),
        opened_at: as_iso(row.created_ms),
        lede: lede.map(|e| e.body.clone()).unwrap_or_default(),
        agent: agent.clone(),
        artifact_id: row.artifact_id.clone(),
        sections,
        updates,
        comms,
        last_seq: row.last_seq,
        read_seq: row.read_seq,
        unseen_count: entries
            .iter()
            .filter(|e| e.seq > row.read_seq && e.kind != "lede")
            .count(),
        last_swept_at: nullable_iso(row.last_swept_ms),
    }
}

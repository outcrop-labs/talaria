// inbox-focus-timeline.ts — the Inbox conversation timeline builder.
//
// Shared by the server (row → entry mapping, context dividers) and mirrored
// client-side for page merges; this port is the server half. Sorting is by
// createdAt then id, both plain string compares — ISO timestamps and uuids are
// fixed-width ASCII, where `localeCompare` and byte order agree.

use serde_json::Value;

use crate::inbox_focus_types::{
    ActivityEntry, ContextEntry, FocusContext, InboxTimelineEntry, MessageEntry,
};

/// One row of the unioned timeline query — a message or a decision, exactly
/// the two shapes `timelineRows` selects.
#[derive(Debug, Clone)]
pub enum TimelineRecord {
    Message {
        id: String,
        created_at: String,
        role: String,
        content: String,
        status: String,
        metadata: Option<Value>,
        attachments: Option<Value>,
    },
    Decision(TimelineDecisionRecord),
}

#[derive(Debug, Clone)]
pub struct TimelineDecisionRecord {
    pub id: String,
    pub created_at: String,
    pub status: String,
    pub action_id: Option<String>,
    #[allow(dead_code)] // selected by timelineRows; the entry reads it from proposal/outcome
    pub instruction: Option<String>,
    pub proposal: Value,
    pub outcome: Value,
    pub focus: FocusContext,
    pub confirmation_token: Option<String>,
    pub expires_at: Option<String>,
    pub undo_expires_at: Option<String>,
}

/// TS `object()`: a JSON object, or null for anything else (arrays included).
fn object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

/// `focusFromMetadata`: the metadata's focus object iff it has the three
/// string fields a divider needs. Returned VERBATIM (extra keys ride along,
/// like the TS cast) — writers emit exactly {key, question, sourceHref,
/// sourceType, sourceId}.
pub fn focus_from_metadata(metadata: Option<&Value>) -> Option<FocusContext> {
    let focus = object(metadata?.get("focus")?)?;
    if focus.get("key").and_then(Value::as_str).is_none()
        || focus.get("question").and_then(Value::as_str).is_none()
        || focus.get("sourceHref").and_then(Value::as_str).is_none()
    {
        return None;
    }
    Some(Value::Object(focus.clone()))
}

/// The focus key a record carries, for divider bookkeeping.
fn focus_key(focus: &Value) -> Option<&str> {
    focus.get("key").and_then(Value::as_str)
}

/// `activityFor` — a decision row into its activity entry.
pub fn activity_for(record: &TimelineDecisionRecord) -> InboxTimelineEntry {
    let proposal = object(&record.proposal);
    let outcome = object(&record.outcome);
    let preview = proposal.and_then(|p| object(p.get("preview")?));
    let undone = outcome.is_some_and(|o| o.get("undone") == Some(&Value::Bool(true)));
    let activity = if undone {
        "undo"
    } else {
        match record.status.as_str() {
            "confirmed" | "failed" => "failure",
            "cancelled" => "cancellation",
            "completed" => "completion",
            _ => {
                if record.confirmation_token.is_some() {
                    "confirmation"
                } else {
                    "proposal"
                }
            }
        }
    };
    let message = proposal
        .and_then(|p| p.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            outcome
                .and_then(|o| o.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            outcome
                .and_then(|o| o.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            (record.status == "confirmed").then(|| {
                "Confirmation was accepted, but the final outcome could not be verified. Open the source before retrying.".to_string()
            })
        });
    let title = match preview.and_then(|p| p.get("title")).and_then(Value::as_str) {
        Some(t) => t.to_string(),
        None => record
            .focus
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    // `preview?.details ?? outcome ?? proposal?.payload ?? null` — first
    // non-nullish, where `outcome` is the whole object.
    let details = preview
        .and_then(|p| p.get("details"))
        .filter(|d| !d.is_null())
        .cloned()
        .or_else(|| outcome.map(|o| Value::Object(o.clone())))
        .or_else(|| {
            proposal
                .and_then(|p| p.get("payload"))
                .filter(|p| !p.is_null())
                .cloned()
        });
    InboxTimelineEntry::Activity(ActivityEntry {
        id: format!("decision:{}", record.id),
        created_at: record.created_at.clone(),
        activity: activity.to_string(),
        decision_id: record.id.clone(),
        focus: record.focus.clone(),
        action_id: record.action_id.clone(),
        title,
        details,
        message,
        confirmation_token: record.confirmation_token.clone(),
        expires_at: record.expires_at.clone(),
        undo_expires_at: record.undo_expires_at.clone(),
    })
}

/// `buildInboxTimeline` — sort, drop the silent clarification decisions, and
/// thread context dividers on focus-key changes.
pub fn build_inbox_timeline(mut records: Vec<TimelineRecord>) -> Vec<InboxTimelineEntry> {
    records.sort_by(|a, b| {
        let ka = (created_of(a), id_of(a));
        let kb = (created_of(b), id_of(b));
        ka.cmp(&kb)
    });
    let mut entries = Vec::with_capacity(records.len());
    let mut last_focus_key: Option<String> = None;
    for record in &records {
        if let TimelineRecord::Decision(d) = record
            && d.action_id.is_none()
            && object(&d.outcome)
                .and_then(|o| o.get("kind"))
                .and_then(Value::as_str)
                == Some("clarification")
        {
            continue;
        }
        let focus = match record {
            TimelineRecord::Message { metadata, .. } => focus_from_metadata(metadata.as_ref()),
            TimelineRecord::Decision(d) => Some(d.focus.clone()),
        };
        match focus_key(focus.as_ref().unwrap_or(&Value::Null)) {
            Some(key) if Some(key) != last_focus_key.as_deref() => {
                entries.push(InboxTimelineEntry::Context(ContextEntry {
                    id: format!("context:{}", id_of(record)),
                    created_at: created_of(record).to_string(),
                    focus: focus.clone().expect("key implies focus"),
                }));
                last_focus_key = Some(key.to_string());
            }
            None => last_focus_key = None,
            _ => {}
        }
        match record {
            TimelineRecord::Message {
                id,
                created_at,
                role,
                content,
                status,
                metadata,
                attachments,
            } => {
                let m = metadata.as_ref();
                entries.push(InboxTimelineEntry::Message(MessageEntry {
                    id: id.clone(),
                    role: role.clone(),
                    content: content.clone(),
                    status: status.clone(),
                    created_at: created_at.clone(),
                    focus: focus.clone(),
                    actor: m
                        .and_then(|m| m.get("actor"))
                        .cloned()
                        .filter(|a| !a.is_null()),
                    delegate_model: m
                        .and_then(|m| m.get("delegateModel"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    response_model: m
                        .and_then(|m| m.get("responseModel"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    mode: m
                        .and_then(|m| m.get("mode"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    attachments: attachments.clone().unwrap_or(Value::Array(vec![])),
                }));
            }
            TimelineRecord::Decision(d) => entries.push(activity_for(d)),
        }
    }
    entries
}

fn created_of(r: &TimelineRecord) -> &str {
    match r {
        TimelineRecord::Message { created_at, .. } => created_at,
        TimelineRecord::Decision(d) => &d.created_at,
    }
}

fn id_of(r: &TimelineRecord) -> &str {
    match r {
        TimelineRecord::Message { id, .. } => id,
        TimelineRecord::Decision(d) => &d.id,
    }
}

// ── Cursors ──────────────────────────────────────────────────────────────────

/// `encodeInboxTimelineCursor` — encodeURIComponent of `createdAt|id`. The
/// unreserved set is JS's exact one (note `'`, `!`, `~`, `(`, `)` ride
/// through; `:` does not, which is why ISO timestamps come out escaped).
pub fn encode_inbox_timeline_cursor(created_at: &str, id: &str) -> String {
    encode_uri_component(&format!("{created_at}|{id}"))
}

fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `decodeInboxTimelineCursor` — the strict cousin: malformed escapes and
/// invalid UTF-8 are the throws TS catches into null. Timestamp validation is
/// RFC3339 rather than `Date.parse` — the recorded divergence is hand-crafted
/// cursors TS would accept ('2026'), which the SQL could never satisfy
/// meaningfully anyway.
pub fn decode_inbox_timeline_cursor(cursor: &str) -> Option<(String, String)> {
    let decoded = decode_uri_component(cursor)?;
    let split = decoded.rfind('|')?;
    if split == 0 {
        return None;
    }
    let created_at = &decoded[..split];
    let id = &decoded[split + 1..];
    if id.is_empty() || !looks_like_timestamp(created_at) {
        return None;
    }
    Some((created_at.to_string(), id.to_string()))
}

fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hi = hex_val(*bytes.get(i + 1)?)?;
                let lo = hex_val(*bytes.get(i + 2)?)?;
                out.push(hi * 16 + lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn looks_like_timestamp(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decision(id: &str, created_at: &str, outcome: Value) -> TimelineRecord {
        TimelineRecord::Decision(TimelineDecisionRecord {
            id: id.to_string(),
            created_at: created_at.to_string(),
            status: "completed".to_string(),
            action_id: None,
            instruction: None,
            proposal: json!({ "message": "done" }),
            outcome,
            focus: json!({ "key": "task:1", "question": "Ship?", "sourceHref": "/x" }),
            confirmation_token: None,
            expires_at: None,
            undo_expires_at: None,
        })
    }

    #[test]
    fn sorts_then_threads_dividers() {
        let records = vec![
            decision("b", "2026-01-02T00:00:00.000Z", json!({ "kind": "other" })),
            decision("a", "2026-01-01T00:00:00.000Z", json!({ "kind": "other" })),
        ];
        let entries = build_inbox_timeline(records);
        // ONE divider for the shared focus key, then an activity per decision,
        // oldest first — the key did not change between the two, so the second
        // does not re-divider.
        assert_eq!(entries.len(), 3);
        assert!(matches!(&entries[0], InboxTimelineEntry::Context(c) if c.id == "context:a"));
        assert!(matches!(&entries[1], InboxTimelineEntry::Activity(a) if a.decision_id == "a"));
    }

    #[test]
    fn clarification_without_action_is_silent() {
        let records = vec![decision(
            "a",
            "2026-01-01T00:00:00.000Z",
            json!({ "kind": "clarification" }),
        )];
        assert!(build_inbox_timeline(records).is_empty());
    }

    #[test]
    fn cursor_round_trip() {
        let enc = encode_inbox_timeline_cursor("2026-01-01T00:00:00.000Z", "abc");
        assert_eq!(enc, "2026-01-01T00%3A00%3A00.000Z%7Cabc");
        assert_eq!(
            decode_inbox_timeline_cursor(&enc),
            Some(("2026-01-01T00:00:00.000Z".to_string(), "abc".to_string()))
        );
        assert_eq!(
            decode_inbox_timeline_cursor("2026-01-01T00:00:00.000Z"),
            None
        );
        assert_eq!(decode_inbox_timeline_cursor("%zz"), None);
        assert_eq!(decode_inbox_timeline_cursor("x|%FF"), None);
        assert_eq!(
            decode_inbox_timeline_cursor("2026-01-01T00:00:00.000Z%7C"),
            None,
            "empty id"
        );
    }
}

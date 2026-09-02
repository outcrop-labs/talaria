// The wire shapes of the Inbox focus family.
//
// Every struct here serializes in declaration order; that order is part of
// the byte contract. Shapes with CONDITIONAL fields (built per branch) do
// not get a struct at all — the engine builds them with `json!`, whose
// `preserve_order` map reproduces each site's key order exactly.

use serde::Serialize;
use serde_json::Value;

/// The four provenances a queue item can have, in the order the queue
/// builder concatenates them.
pub const FOCUS_SOURCE_TYPES: [&str; 4] = ["approval", "task", "channel", "notification"];

/// One evidence line on a queue card. Reused from the harness def, which owns
/// the camelCase shape the model's prompt and the card both speak.
pub use crate::harness::defs::inbox_focus::{FocusAction, FocusEvidence};

/// The queue card exactly as the client renders it (FocusItem). `metadata` is
/// an open record per source — kept as a Value so each source writes its own
/// keys in its own order.
#[derive(Debug, Clone)]
pub struct RawFocusItem {
    pub key: String,
    pub source_type: String,
    pub source_id: String,
    pub priority: String,
    pub status_label: String,
    pub created_at: String,
    pub updated_at: String,
    pub due_at: Option<String>,
    pub question: String,
    pub recommendation: String,
    pub recommended_action_id: Option<String>,
    pub evidence: Vec<FocusEvidence>,
    pub metadata: Value,
    pub source_href: String,
    pub brief_status: String,
    pub actions: Vec<FocusAction>,
    // RawFocusItem's two extensions — engine-only, never serialized to the
    // client (the queue response builds wire items without them).
    pub bucket: i64,
    pub source_fingerprint: String,
}

/// RawFocusItem → the FocusItem wire object, in declaration order. A fn
/// rather than a derive so the two engine-only extensions can never leak onto
/// the wire by accident.
pub fn wire_item(item: &RawFocusItem) -> Value {
    serde_json::to_value(wire_item_struct(item)).expect("raw focus item serializes")
}

/// The typed half of `wire_item` — declaration order, no skips: every field
/// serializes on every card.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FocusItemWire<'a> {
    key: &'a str,
    source_type: &'a str,
    source_id: &'a str,
    priority: &'a str,
    status_label: &'a str,
    created_at: &'a str,
    updated_at: &'a str,
    due_at: Option<&'a str>,
    question: &'a str,
    recommendation: &'a str,
    recommended_action_id: Option<&'a str>,
    evidence: &'a [FocusEvidence],
    metadata: &'a Value,
    source_href: &'a str,
    brief_status: &'a str,
    actions: &'a [FocusAction],
}

fn wire_item_struct(item: &RawFocusItem) -> FocusItemWire<'_> {
    FocusItemWire {
        key: &item.key,
        source_type: &item.source_type,
        source_id: &item.source_id,
        priority: &item.priority,
        status_label: &item.status_label,
        created_at: &item.created_at,
        updated_at: &item.updated_at,
        due_at: item.due_at.as_deref(),
        question: &item.question,
        recommendation: &item.recommendation,
        recommended_action_id: item.recommended_action_id.as_deref(),
        evidence: &item.evidence,
        metadata: &item.metadata,
        source_href: &item.source_href,
        brief_status: &item.brief_status,
        actions: &item.actions,
    }
}

/// The five bucket counts under the queue (FocusCounts).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusCounts {
    pub total: usize,
    pub approvals: usize,
    pub tasks: usize,
    pub comms: usize,
    pub notifications: usize,
}

/// Whether the owner has an Inbox assistant persona at all (FocusAssistant).
/// `configured` is a separate boolean rather than "model is Some" because the
/// panel renders three states: no assistant, unnamed assistant, named one.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusAssistant {
    pub configured: bool,
    pub model: Option<String>,
    pub name: Option<String>,
}

/// The one-line brief a queue card shows (AssistantBrief), and the jsonb the
/// brief cache stores. `recommendedActionId` is clamped by `valid_brief` to an
/// action the card actually offers.
#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssistantBrief {
    pub question: String,
    pub recommendation: String,
    pub recommended_action_id: Option<String>,
}

// ── The timeline ─────────────────────────────────────────────────────────────

/// An entry's focus context. Kept as the ORIGINAL Value (not a struct)
/// because two spellings exist on the wire and both must round-trip
/// unchanged: fresh contexts are built in literal key order, DB contexts
/// come back in PostgreSQL's jsonb key order. `focus_of` is the validator
/// both producers share.
pub type FocusContext = Value;

/// One timeline message entry (the `message` variant). `attachments` is
/// always an array, possibly empty.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: String,
    pub role: String,
    pub content: String,
    pub status: String,
    pub created_at: String,
    pub focus: Option<FocusContext>,
    pub actor: Option<Value>,
    pub delegate_model: Option<String>,
    pub response_model: Option<String>,
    pub mode: Option<String>,
    pub attachments: Value,
}

/// A context divider (the `context` variant) — the anchor a conversation's
/// question hangs under.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEntry {
    pub id: String,
    pub created_at: String,
    pub focus: FocusContext,
}

/// A decision's timeline record (the `activity` variant). The three
/// confirmation fields are conditional — `skip_serializing_if` keeps them
/// absent when None.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub created_at: String,
    pub activity: String,
    pub decision_id: String,
    pub focus: FocusContext,
    pub action_id: Option<String>,
    pub title: String,
    pub details: Option<Value>,
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_expires_at: Option<String>,
}

/// Internally tagged with `kind`, variant fields in declaration order;
/// `kind` serializes first (the tag).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InboxTimelineEntry {
    Message(MessageEntry),
    Context(ContextEntry),
    Activity(ActivityEntry),
}

/// One page of the conversation timeline (InboxConversationPage).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxConversationPage {
    pub conversation_id: Option<String>,
    pub entries: Vec<InboxTimelineEntry>,
    pub next_cursor: Option<String>,
    pub working: bool,
}

/// The command SSE stream's frame payloads (InboxCommandEvent), tagged with
/// `type`. `done.result` is the FocusCommandResponse — one of two shapes, so
/// it stays a Value. `rename_all` camelCases the variant TAGS only; the
/// fields inside the variants need `rename_all_fields` — without it
/// `conversation_id` rode the wire snake_case and the panel's reader
/// (which reads `conversationId`) lost the id on every frame.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InboxCommandEvent {
    Conversation {
        conversation_id: String,
        entry: InboxTimelineEntry,
    },
    Status {
        label: String,
    },
    Content {
        text: String,
    },
    Activity {
        entry: InboxTimelineEntry,
    },
    Done {
        conversation_id: String,
        entry: InboxTimelineEntry,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    Error {
        message: String,
    },
}

// ── The ad-hoc result shapes ─────────────────────────────────────────────────
//
// The engine's per-site action/command results have NO fixed key order: each
// branch spells its own literal (some put `message` before `decisionId`, the
// catch paths after it), so they are built per site as Values via `json!`.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The SSE frame payloads are read by the panel's `conversationId` key —
    /// pinned here because the enum's `rename_all` camelCases only the variant
    /// tags, and a regression to snake_case fields is invisible to the Rust
    /// compiler (the field name is legal either way).
    #[test]
    fn command_events_serialize_camel_case() {
        let entry = InboxTimelineEntry::Message(MessageEntry {
            id: "m1".into(),
            role: "user".into(),
            content: "hello".into(),
            status: "complete".into(),
            created_at: "2026-08-29T00:00:00.000Z".into(),
            focus: None,
            actor: None,
            delegate_model: None,
            response_model: None,
            mode: None,
            attachments: json!([]),
        });
        let conversation = serde_json::to_value(InboxCommandEvent::Conversation {
            conversation_id: "c1".into(),
            entry: entry.clone(),
        })
        .expect("serializes");
        assert_eq!(
            conversation,
            json!({ "type": "conversation", "conversationId": "c1", "entry": conversation["entry"] })
        );

        let done = serde_json::to_value(InboxCommandEvent::Done {
            conversation_id: "c1".into(),
            entry,
            result: Some(json!({ "ok": true })),
        })
        .expect("serializes");
        assert_eq!(done["type"], "done");
        assert_eq!(done["conversationId"], "c1");
        assert_eq!(done["result"]["ok"], true);
    }
}

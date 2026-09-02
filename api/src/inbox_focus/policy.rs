// The Inbox focus policy — the inbox-coupled half. The prompts, surface map,
// history cap and `validate_command_object` live with the harness def
// (`harness/defs/inbox_focus.rs`); the brief-shared helpers (fingerprint, key,
// task wording) live with the brief (`daily_brief/focus.rs`). What remains
// here is what only the Inbox queue reads: the action model, the
// proposal-source gate, and the deterministic instruction match.

use serde_json::{Value, json};

use crate::body::truncate_utf16;
use crate::daily_brief::focus::fingerprint;
use crate::harness::defs::inbox_focus::FocusAction;
use crate::inbox_focus::types::{AssistantBrief, RawFocusItem};

// These spellings are the brief's helpers, re-exported so the sources module
// names one path.
pub use crate::daily_brief::focus::{
    ACTIONABLE_NOTIFICATION_KINDS, as_iso, key_of, nullable_iso, priority_for_bucket, task_bucket,
    task_question, task_recommendation, task_status_label,
};

/// The one constructor every source builds its card actions through, so the
/// two derived booleans can never disagree with the risk.
pub fn focus_action(
    id: &str,
    label: &str,
    risk: &str,
    confirmation_required: Option<bool>,
    reversible: Option<bool>,
) -> FocusAction {
    FocusAction {
        id: id.to_string(),
        label: label.to_string(),
        risk: risk.to_string(),
        confirmation_required: confirmation_required.unwrap_or(risk == "confirmation"),
        reversible: reversible.unwrap_or(risk == "reversible"),
    }
}

// ── Who proposed it, which is now half of whether it may run ─────────────────

/// Where a proposed action came from (FocusProposalSource). The security
/// posture lives in `requires_human_confirmation`: an ALLOWLIST of the two
/// sources that may skip the click, so a source added later needs a deliberate
/// edit there to become auto-executable rather than inheriting it by omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusProposalSource {
    /// A person clicked a button on the card, or Retry on a past decision.
    Human,
    /// `deterministic_proposal`'s regexes matched the owner's instruction and
    /// this is the action they matched.
    Deterministic,
    /// A model selected this action from the item's full list (the widened
    /// surface) — nothing deterministic authorized it.
    Widened,
    /// The delegate seat's proposal carried the turn.
    Delegate,
    /// A proposal row this build cannot read a source off. Needs the click.
    Unattributed,
}

impl FocusProposalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Deterministic => "deterministic",
            Self::Widened => "widened",
            Self::Delegate => "delegate",
            Self::Unattributed => "unattributed",
        }
    }

    fn of(raw: &str) -> Option<Self> {
        Some(match raw {
            "human" => Self::Human,
            "deterministic" => Self::Deterministic,
            "widened" => Self::Widened,
            "delegate" => Self::Delegate,
            "unattributed" => Self::Unattributed,
            _ => return None,
        })
    }
}

/// Label a model's proposal from the proposal ITSELF (the action id it named
/// vs the one the regexes matched) rather than from the runner's `widened`
/// flag. `payload` is deliberately not compared.
pub fn proposal_source_for(
    from_delegate: bool,
    action_id: &str,
    deterministic_action_id: Option<&str>,
) -> FocusProposalSource {
    if from_delegate {
        return FocusProposalSource::Delegate;
    }
    if deterministic_action_id == Some(action_id) {
        FocusProposalSource::Deterministic
    } else {
        FocusProposalSource::Widened
    }
}

/// Read the source back off a persisted `inbox_decisions.proposal`.
/// Unrecognized is `Unattributed`, never a guess.
pub fn proposal_source_of(proposal: &Value) -> FocusProposalSource {
    proposal
        .as_object()
        .and_then(|p| p.get("source"))
        .and_then(Value::as_str)
        .and_then(FocusProposalSource::of)
        .unwrap_or(FocusProposalSource::Unattributed)
}

/// THE GATE. Must this action be shown to a human before it runs?
/// `action.confirmation_required` still wins on its own; this only ever ADDS a
/// confirmation.
pub fn requires_human_confirmation(action: &FocusAction, source: FocusProposalSource) -> bool {
    if action.confirmation_required {
        return true;
    }
    !matches!(
        source,
        FocusProposalSource::Human | FocusProposalSource::Deterministic
    )
}

/// A held token stops being valid when the proposal expired or the item moved
/// underneath it. An unparseable expiry reads as expired.
pub fn confirmation_miss_invalidates(
    status: Option<&str>,
    expires_at: Option<&str>,
    stored_fingerprint: Option<&str>,
    current_fingerprint: &str,
    now_ms: i64,
) -> bool {
    if status != Some("proposed") {
        return false;
    }
    let expired = match expires_at.filter(|e| !e.is_empty()) {
        None => true,
        Some(raw) => match crate::agent_auth::iso_to_epoch_ms(raw) {
            Some(ms) => ms <= now_ms,
            None => true,
        },
    };
    expired || stored_fingerprint != Some(current_fingerprint)
}

/// Clamp the model's one-liner to what the card renders, and drop a
/// recommendedActionId the card does not offer.
pub fn valid_brief(value: Option<&Value>, actions: &[FocusAction]) -> Option<AssistantBrief> {
    let obj = value?.as_object()?;
    let question = obj.get("question")?.as_str()?;
    let recommendation = obj.get("recommendation")?.as_str()?;
    let proposed = obj.get("recommendedActionId").and_then(Value::as_str);
    Some(AssistantBrief {
        question: truncate_utf16(question, 240).to_string(),
        recommendation: truncate_utf16(recommendation, 500).to_string(),
        recommended_action_id: proposed
            .filter(|p| actions.iter().any(|a| a.id == *p))
            .map(str::to_string),
    })
}

/// The fingerprint over the INBOX item shape (the brief's twin reads its own
/// shape in daily_brief/focus.rs). `updatedAt` is deliberately excluded — a
/// touch that changes nothing the card shows must not invalidate a held
/// confirmation.
fn item_fingerprint(item: &RawFocusItem) -> String {
    fingerprint(&json!({
        "sourceType": item.source_type,
        "sourceId": item.source_id,
        "question": item.question,
        "evidence": item.evidence.iter().map(|e| json!({"label": e.label, "text": e.text})).collect::<Vec<_>>(),
        "actions": item.actions.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        "statusLabel": item.status_label,
        "dueAt": item.due_at,
    }))
}

/// Stamp the fingerprint once; every later comparison reads it.
pub fn finalize_item(mut item: RawFocusItem) -> RawFocusItem {
    item.source_fingerprint = item_fingerprint(&item);
    item
}

/// A notification that merely points at a task or channel already in the
/// set goes; then one entry per link. The brief's twin lives in
/// daily_brief/focus.rs over its own item shape.
pub fn dedupe_items(items: Vec<RawFocusItem>) -> Vec<RawFocusItem> {
    let task_ids: std::collections::HashSet<String> = items
        .iter()
        .filter(|i| i.source_type == "task")
        .map(|i| i.source_id.clone())
        .collect();
    let channel_ids: std::collections::HashSet<String> = items
        .iter()
        .filter(|i| i.source_type == "channel")
        .map(|i| i.source_id.clone())
        .collect();
    let mut seen_links: std::collections::HashSet<String> = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| {
            if item.source_type == "notification" {
                if task_ids
                    .iter()
                    .any(|id| item.source_href.contains(id.as_str()))
                {
                    return false;
                }
                if channel_ids
                    .iter()
                    .any(|id| item.source_href.contains(id.as_str()))
                {
                    return false;
                }
            }
            let link_key = if !item.source_href.is_empty() && item.source_href != "/" {
                item.source_href.clone()
            } else {
                item.key.clone()
            };
            if seen_links.contains(&link_key) && item.source_type == "notification" {
                return false;
            }
            seen_links.insert(link_key);
            true
        })
        .collect()
}

/// Bucket, due-ness, explicit priority, age — a stable sort.
pub fn sort_items(mut items: Vec<RawFocusItem>, now_ms: i64) -> Vec<RawFocusItem> {
    let due_rank = |item: &RawFocusItem| -> i64 {
        let Some(raw) = item.due_at.as_deref() else {
            return 2;
        };
        // An unparseable due date is NaN — every comparison on it is false,
        // landing it in "due, but not soon".
        let Some(due) = crate::agent_auth::iso_to_epoch_ms(raw) else {
            return 1;
        };
        if due <= now_ms + 7 * 86_400_000 { 0 } else { 1 }
    };
    let priority_rank = |p: &str| -> i64 {
        match p {
            "urgent" => 0,
            "high" => 1,
            "medium" => 2,
            "low" => 3,
            // An absent metadata.priority ranks at 4.
            _ => 4,
        }
    };
    let explicit_rank = |item: &RawFocusItem| -> i64 {
        priority_rank(
            item.metadata
                .get("priority")
                .and_then(Value::as_str)
                .unwrap_or("undefined"),
        )
    };
    items.sort_by(|a, b| {
        a.bucket
            .cmp(&b.bucket)
            .then_with(|| due_rank(a).cmp(&due_rank(b)))
            .then_with(|| explicit_rank(a).cmp(&explicit_rank(b)))
            .then_with(|| created_ms(a).cmp(&created_ms(b)))
    });
    items
}

fn created_ms(item: &RawFocusItem) -> i64 {
    crate::agent_auth::iso_to_epoch_ms(&item.created_at).unwrap_or(0)
}

// ── The deterministic instruction match ──────────────────────────────────────

/// What a matched instruction authorizes.
#[derive(Debug, Clone)]
pub struct CommandProposal {
    pub action_id: String,
    pub message: String,
    pub payload: Option<Value>,
}

/// The owner's own words, matched exactly. A model that echoes the one id it
/// was shown adds no authority to a typed "approve it", and no model at all
/// is needed to hear one.
pub fn deterministic_proposal(item: &RawFocusItem, instruction: &str) -> Option<CommandProposal> {
    let lower = instruction.trim().to_lowercase();
    let available = |id: &str| item.actions.iter().any(|a| a.id == id);
    let label_of = |id: &str| {
        item.actions
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.label.to_lowercase())
    };
    let direct: [(&str, &str); 3] = [
        // approve / reject-decline / mark-read, with optional "please", an
        // optional "this|the|it" and an optional object noun. Fully anchored.
        (
            r"^(?:please\s+)?approve(?:\s+(?:this|the|it))?(?:\s+(?:task|work|request|approval|item|draft))?[.!]?$",
            if item.source_type == "task" {
                "approve_task"
            } else {
                "approve"
            },
        ),
        (
            r"^(?:please\s+)?(?:reject|decline)(?:\s+(?:this|the|it))?(?:\s+(?:task|work|request|approval|item|draft))?[.!]?$",
            if item.source_type == "task" {
                "request_changes"
            } else {
                "reject"
            },
        ),
        (
            r"^(?:please\s+)?(?:mark|set)(?:\s+(?:this|the|it))?(?:\s+(?:item|message|notification|conversation))?\s+(?:as\s+)?read[.!]?$",
            "mark_read",
        ),
    ];
    for (pattern, action_id) in direct {
        let re = regex::Regex::new(pattern).expect("deterministic patterns compile");
        if re.is_match(&lower) && available(action_id) {
            let label = label_of(action_id).unwrap_or_else(|| action_id.to_string());
            return Some(CommandProposal {
                action_id: action_id.to_string(),
                message: format!("Ready to {label}."),
                payload: None,
            });
        }
    }
    if available("reply") {
        let re = regex::Regex::new(
            r"(?i)^\s*(?:reply|respond|tell (?:them|him|her))\s*(?::|with|that)?\s+(?s:(.+))$",
        )
        .expect("reply pattern compiles");
        if let Some(m) = re
            .captures(instruction)
            .and_then(|caps| caps.get(1))
            .map(|g| g.as_str().trim())
            .filter(|s| !s.is_empty())
        {
            return Some(CommandProposal {
                action_id: "reply".to_string(),
                message: "I prepared this reply for confirmation.".to_string(),
                payload: Some(json!({ "message": truncate_utf16(m, 20_000) })),
            });
        }
    }
    None
}

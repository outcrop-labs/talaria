// Slot-level reasoning-effort preferences — the admin's "how hard should this
// CLASS of work think" dial. Port of effort-prefs.ts.
//
// THE SHAPE. One settings row, keys are the SLOT the turn ran under:
//   'role:<ModelRole>'    when the model chain's role step (or the utility
//                         step) produced the model
//   'agent:<PlatformAgentId>'
//                         when the pin step produced it
// The value is the effort string. Slots with no preference are absent —
// absent means "the model's own default", which is also what the empty string
// means everywhere an effort is offered, so there is exactly one spelling of
// "no preference" on either side of the wire.
//
// VALIDATION IS AT RUN TIME, deliberately. The preference is stored as the
// admin set it and held against the model's LIVE published levels when a turn
// is about to carry it: a level the model stopped publishing is dropped, not
// sent. The write paths also validate (a typo in a settings row should
// bounce), but the run-time check is the one correctness depends on —
// assignments and catalogs move under a stored preference, and the turn must
// degrade rather than 400.

use crate::gateway::settings::{get_setting, set_setting};
use sqlx::PgPool;

const KEY: &str = "effort_prefs";

pub fn role_slot(role: &str) -> String {
    format!("role:{role}")
}

pub fn agent_slot(id: &str) -> String {
    format!("agent:{id}")
}

/// The filtered view: string efforts, trimmed non-empty, slots ≤ 64 UTF-16
/// units; values stored trimmed. Iteration order is the stored object's
/// (preserve_order), because the write path round-trips this map.
pub async fn get_effort_prefs(pg: &PgPool) -> serde_json::Map<String, serde_json::Value> {
    let stored = get_setting(pg, KEY, serde_json::json!({})).await;
    let mut out = serde_json::Map::new();
    if let Some(obj) = stored.as_object() {
        for (slot, effort) in obj {
            if let Some(e) = effort.as_str()
                && !e.trim().is_empty()
                && slot.encode_utf16().count() <= 64
            {
                out.insert(
                    slot.clone(),
                    serde_json::Value::String(e.trim().to_string()),
                );
            }
        }
    }
    out
}

/// Set or clear one slot. A falsy effort (null, absent, empty/whitespace —
/// the TS write route's `model: ""` spelling of "clear") deletes the slot.
pub async fn set_effort_pref(
    pg: &PgPool,
    slot: &str,
    effort: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut cur = get_effort_prefs(pg).await;
    match effort.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => {
            cur.insert(slot.to_string(), serde_json::Value::String(e.to_string()));
        }
        None => {
            cur.remove(slot);
        }
    }
    let value = serde_json::Value::Object(cur);
    set_setting(pg, KEY, &value).await
}

/// The stored preference for a slot, held against the model's live published
/// levels — the run-time half, and the one the runner's precedence rule
/// depends on. None when there is no preference, or when the preferred level
/// is not one the model currently publishes: a stale preference is no
/// preference. Never fails — this exists to improve a turn, not to gate one.
pub async fn slot_effort_for_model(pg: &PgPool, slot: &str, model: &str) -> Option<String> {
    let stored = get_effort_prefs(pg).await.get(slot)?.as_str()?.to_string();
    let levels = crate::model::efforts::efforts_for_model(pg, model).await;
    levels.contains(&stored).then_some(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_and_shapes() {
        assert_eq!(role_slot("utility"), "role:utility");
        assert_eq!(agent_slot("muse"), "agent:muse");
        // 64 is a UTF-16 budget: a slot of 64 one-unit chars is legal.
        let s64 = "s".repeat(64);
        assert_eq!(s64.encode_utf16().count(), 64);
        let s65 = "s".repeat(65);
        assert!(s65.encode_utf16().count() > 64);
    }
}

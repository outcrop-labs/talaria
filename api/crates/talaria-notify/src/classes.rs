// The class vocabulary: which classes a person has an opinion about, and
// which class each writer's `kind` belongs to. Declared here once; prefs and
// the digest DERIVE the effective answer from it.

/// One class as declared above — id and the fallback route
/// a person who never touched the switch gets. Order is the wire order: the
/// resolved prefs object serializes in this sequence.
pub const NOTIFY_CLASSES: &[(&str, &str)] = &[
    ("mention", "both"),
    ("dm", "both"),
    ("approval_pending", "both"),
    ("judge_escalation", "both"),
    ("agent_blocked", "both"),
    ("gap_reported", "in_app"),
    ("work_complete", "in_app"),
    ("workchain_turn", "both"),
    ("workchain_paused", "in_app"),
];

/// `kind` (what the writer called it) → the class a user has an opinion about.
/// Kinds are free-form strings at ~10 call sites and predate this table; the
/// map is how they stay free-form without every new one inventing a setting.
fn kind_class(kind: &str) -> Option<&'static str> {
    Some(match kind {
        // Someone pointed at you on purpose.
        "mention" | "kb-comment" | "task-assigned" | "plan-share" | "research-share" => "mention",
        // Addressed to you, in a thread of your own.
        "dm" | "agent-outreach" | "agent-reply" => "dm",
        // Blocked on a human.
        "agent-problem" | "workbench-repo-request" => "agent_blocked",
        // Outcomes.
        "research" | "task-status" | "board_access" => "work_complete",
        // A capability agents lost: an MCP server the health pass saw go
        // from ok to down. In-app by default — an admin fixes it from the
        // MCP page; nothing is parked waiting on this minute.
        "mcp_server_down" => "gap_reported",
        // `judge_escalation` and `gap_reported` are NOT here on purpose: their
        // writers name the CLASS as the kind, which `notify_class_of` accepts
        // directly. A kind that is already the class it belongs to has nothing
        // to map.
        _ => return None,
    })
}

/// The class a notification belongs to. Every class id is also
/// accepted as a kind, so a new writer can name the class directly and skip
/// the table.
///
/// An UNRECOGNIZED kind lands in `work_complete` — the quiet bucket — on
/// purpose: a notification kind added by someone who never opened this file
/// must not start mailing the whole org because it fell through a default.
pub fn notify_class_of(kind: &str) -> &'static str {
    kind_class(kind).unwrap_or_else(|| {
        // The class table's OWN static id, not the caller's borrow — the ids
        // are the same bytes, and only one of them can be returned as
        // &'static.
        NOTIFY_CLASSES
            .iter()
            .find(|(id, _)| *id == kind)
            .map(|(id, _)| *id)
            .unwrap_or("work_complete")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_route_to_their_classes() {
        // The pinned arms, one line per family, then the fall-through rule.
        for (kind, class) in [
            ("mention", "mention"),
            ("kb-comment", "mention"),
            ("plan-share", "mention"),
            ("dm", "dm"),
            ("agent-outreach", "dm"),
            // A reply an agent filed in your thread — yours, like a DM. The
            // arm exists ahead of its writer; routing lands first so the first
            // row ever filed is already in the right bucket.
            ("agent-reply", "dm"),
            ("agent-problem", "agent_blocked"),
            ("research", "work_complete"),
            ("task-status", "work_complete"),
            // An access request's outcome — the request was yours, but the
            // notification is news, not a door. Explicit so the quiet bucket
            // is a decision here, not the fall-through's default.
            ("board_access", "work_complete"),
            // The MCP health pass's ok → down notification: in-app, like
            // the capability gaps it is one of.
            ("mcp_server_down", "gap_reported"),
            // Every class id is its own kind.
            ("judge_escalation", "judge_escalation"),
            ("gap_reported", "gap_reported"),
        ] {
            assert_eq!(notify_class_of(kind), class, "kind {kind}");
        }
        // An unknown kind lands in the quiet bucket, never the mail-defaults.
        assert_eq!(notify_class_of("brand-new-kind"), "work_complete");
    }
}

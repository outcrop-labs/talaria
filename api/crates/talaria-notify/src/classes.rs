// The class vocabulary: which classes a person has an opinion about, which
// class each writer's `kind` belongs to, and the registry the wire serves as
// `classes[]` (I-12). Declared here once; prefs and the digest DERIVE the
// effective answer from it, and the client renders what it is served — the
// labels and blurbs live here, not in the bundle.
//
// scripts/check-invariants.mjs reads KIND_CLASS: every `kind: "<literal>"` a
// `NotificationInput` files must be a kind below or a class id.

use serde_json::{Value, json};

/// The ORIGINAL classes and the fallback route a person who never touched the
/// switch gets — wave 1's legacy table, kept byte for byte. Order is the wire
/// order of the legacy `prefs` object.
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

/// The classes the digest's derived answer reads (C27). Frozen: the first new
/// class must not flip a person's digest on by existing.
pub const ORIGINAL_CLASSES: &[&str] = &[
    "mention",
    "dm",
    "approval_pending",
    "judge_escalation",
    "agent_blocked",
    "gap_reported",
    "work_complete",
    "workchain_turn",
    "workchain_paused",
];

/// `kind` (what the writer called it) → (the class a person has an opinion
/// about, the reason a row of this kind shows). Kinds are free-form strings at
/// ~20 call sites and predate this table; the map is how they stay free-form
/// without every new one inventing a setting. A class id is also accepted as a
/// kind (`notify_class_of`); the ones with writers are listed so the wire can
/// name their reason.
pub const KIND_CLASS: &[(&str, &str, &str)] = &[
    // Blocked on a human.
    ("approval_pending", "approval_pending", "Approval waiting"),
    ("judge_escalation", "judge_escalation", "Escalated to you"),
    ("agent-problem", "agent_blocked", "Agent stopped"),
    (
        "workbench-repo-request",
        "agent_blocked",
        "Repository request",
    ),
    ("workchain_turn", "workchain_turn", "Your turn"),
    // Someone pointed at you on purpose.
    ("mention", "mention", "Mentioned you"),
    ("kb-comment", "mention", "Commented on your doc"),
    ("task-assigned", "mention", "Assigned to you"),
    ("plan-share", "mention", "Shared a plan"),
    ("research-share", "mention", "Shared research"),
    // Addressed to you, in a thread of your own.
    ("dm", "dm", "Direct message"),
    ("agent-outreach", "dm", "An agent reached out"),
    ("agent-reply", "dm", "Replied in your thread"),
    // You take part (W2-P files these).
    (
        "task-comment",
        "participating",
        "Commented on a ticket you follow",
    ),
    (
        "thread-reply",
        "participating",
        "Replied in a thread you're in",
    ),
    // Outcomes.
    ("research", "work_complete", "Research ready"),
    ("task-status", "work_complete", "Status changed"),
    // An access request's outcome — the request was yours, but the
    // notification is news, not a door.
    ("board_access", "work_complete", "Access decided"),
    // A Google principal for an agent was provisioned. W2-E remaps it to an
    // `account` class; until then it is where the fall-through put it.
    (
        "agent_google_principal",
        "work_complete",
        "Agent Google account",
    ),
    ("workchain_paused", "workchain_paused", "Workchain paused"),
    // A channel at level `all` (W2-P files these).
    ("channel-message", "channel_activity", "New message"),
    // A capability agents lost: an MCP server the health pass saw go from ok
    // to down. In-app by default — an admin fixes it from the MCP page.
    ("gap_reported", "gap_reported", "Capability gap"),
    ("mcp_server_down", "gap_reported", "MCP server down"),
];

/// Where a class sits in the center and the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Somebody is blocked until you act.
    NeedsYou,
    /// Addressed to you.
    Direct,
    /// News about things you follow.
    Following,
    /// The workspace's own health (admins).
    Workspace,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::NeedsYou => "needs_you",
            Category::Direct => "direct",
            Category::Following => "following",
            Category::Workspace => "workspace",
        }
    }
    /// For you = Needs you + Direct (I-12, O-10).
    pub fn for_you(self) -> bool {
        matches!(self, Category::NeedsYou | Category::Direct)
    }
}

/// A class's delivery channels. In-app is always on — the row is the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelDefaults {
    pub desktop: bool,
    pub push: bool,
    pub email: bool,
}

/// One class of the registry.
#[derive(Clone, Copy, Debug)]
pub struct ClassDef {
    pub id: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    pub category: Category,
    /// What a person who never touched the matrix gets. Phase 0: exactly
    /// wave 1's effective answer (push whenever the route kept an in-app copy,
    /// mail when the fallback route was `both`).
    pub defaults: ChannelDefaults,
    /// Alerts through DND (a decision a person is blocking).
    pub breaks_through: bool,
    /// Members never get rows of it, so they are not offered it.
    pub admin_only: bool,
    /// How long the missed-activity mail waits before it sends (W2-E). 0 = the
    /// person's own `emailFrequency`.
    pub email_delay_min: u32,
}

const fn ch(desktop: bool, push: bool, email: bool) -> ChannelDefaults {
    ChannelDefaults {
        desktop,
        push,
        email,
    }
}

/// THE registry, in wire order. The split is by WHO IS BLOCKED, not by
/// importance: mail goes out when a person is the only one who can unblock
/// something; everything else is an outcome they will see the next time they
/// look.
pub const CLASSES: &[ClassDef] = &[
    ClassDef {
        id: "approval_pending",
        label: "Approvals waiting on you",
        blurb: "Work is parked until you approve or reject it.",
        category: Category::NeedsYou,
        defaults: ch(true, true, true),
        breaks_through: true,
        admin_only: false,
        email_delay_min: 15,
    },
    ClassDef {
        id: "judge_escalation",
        label: "Judge escalations",
        blurb: "The quality gate stopped work and handed it to a person.",
        category: Category::NeedsYou,
        defaults: ch(true, true, true),
        breaks_through: true,
        admin_only: false,
        email_delay_min: 15,
    },
    ClassDef {
        id: "agent_blocked",
        label: "Blocked agents",
        blurb: "An agent hit a problem and stopped. It stays stopped until someone looks.",
        category: Category::NeedsYou,
        defaults: ch(true, true, true),
        breaks_through: true,
        admin_only: true,
        email_delay_min: 15,
    },
    ClassDef {
        id: "workchain_turn",
        label: "Your turn in a workchain",
        blurb: "A workchain handed the next step to you.",
        category: Category::NeedsYou,
        defaults: ch(true, true, true),
        breaks_through: true,
        admin_only: false,
        email_delay_min: 15,
    },
    ClassDef {
        id: "mention",
        label: "Mentions & shares",
        blurb: "Someone @mentioned you, assigned you a ticket, or shared a doc with you.",
        category: Category::Direct,
        defaults: ch(true, true, true),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "dm",
        label: "Direct messages",
        blurb: "A teammate or an agent messaged you directly.",
        category: Category::Direct,
        defaults: ch(true, true, true),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "participating",
        label: "Threads you're in",
        blurb: "Replies and comments on tickets and threads you take part in.",
        category: Category::Direct,
        defaults: ch(true, true, false),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "work_complete",
        label: "Work finished",
        blurb: "A ticket moved, or research you asked for is ready.",
        category: Category::Following,
        defaults: ch(true, true, false),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "workchain_paused",
        label: "Workchains paused",
        blurb: "A workchain you run stopped and is waiting.",
        category: Category::Following,
        defaults: ch(true, true, false),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "channel_activity",
        label: "Channel activity",
        blurb: "New messages in channels you set to notify on every message.",
        category: Category::Following,
        defaults: ch(true, false, false),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "gap_reported",
        label: "Capability gaps",
        blurb: "An agent reported something it could not do, or lost a tool, for a human to ratify.",
        category: Category::Workspace,
        defaults: ch(true, true, false),
        breaks_through: false,
        admin_only: true,
        email_delay_min: 0,
    },
];

/// The registry entry for a class id.
pub fn class_def(id: &str) -> Option<&'static ClassDef> {
    CLASSES.iter().find(|c| c.id == id)
}

fn kind_entry(kind: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    KIND_CLASS.iter().find(|(k, _, _)| *k == kind)
}

/// The class a notification belongs to. Every class id is also accepted as a
/// kind, so a new writer can name the class directly and skip the table.
///
/// An UNRECOGNIZED kind lands in `work_complete` — the quiet bucket — on
/// purpose: a notification kind added by someone who never opened this file
/// must not start mailing the whole org because it fell through a default.
pub fn notify_class_of(kind: &str) -> &'static str {
    if let Some((_, class, _)) = kind_entry(kind) {
        return class;
    }
    // The registry's OWN static id, not the caller's borrow — the ids are the
    // same bytes, and only one of them can be returned as &'static.
    class_def(kind).map(|c| c.id).unwrap_or("work_complete")
}

/// Why a row of this kind shows ("Mentioned you"), or None for a kind the
/// table does not name.
pub fn kind_reason(kind: &str) -> Option<&'static str> {
    kind_entry(kind).map(|(_, _, reason)| *reason)
}

/// The category a kind's class sits in.
pub fn category_of(kind: &str) -> Category {
    class_def(notify_class_of(kind))
        .map(|c| c.category)
        .unwrap_or(Category::Following)
}

/// Every kind that lands in a For-you class (Needs you + Direct): the named
/// kinds, plus each such class id (a writer may name the class as its kind).
/// What `unreadForYou` counts with `kind = any($1)`.
pub fn for_you_kinds() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = KIND_CLASS
        .iter()
        .filter(|(_, class, _)| class_def(class).is_some_and(|c| c.category.for_you()))
        .map(|(kind, _, _)| *kind)
        .collect();
    for c in CLASSES.iter().filter(|c| c.category.for_you()) {
        if !out.contains(&c.id) {
            out.push(c.id);
        }
    }
    out
}

/// `classes[]` as `GET /api/notifications` serves it (I-12), in registry
/// order. A member is not offered an admin-only class.
pub fn classes_wire(is_admin: bool) -> Value {
    Value::Array(
        CLASSES
            .iter()
            .filter(|c| is_admin || !c.admin_only)
            .map(|c| {
                let kinds: Vec<Value> = KIND_CLASS
                    .iter()
                    .filter(|(_, class, _)| *class == c.id)
                    .map(|(kind, _, reason)| json!({ "id": kind, "reason": reason }))
                    .collect();
                json!({
                    "id": c.id,
                    "label": c.label,
                    "blurb": c.blurb,
                    "category": c.category.as_str(),
                    "defaults": {
                        "desktop": c.defaults.desktop,
                        "push": c.defaults.push,
                        "email": c.defaults.email,
                    },
                    "breaksThrough": c.breaks_through,
                    "adminOnly": c.admin_only,
                    "emailDelayMin": c.email_delay_min,
                    "kinds": kinds,
                })
            })
            .collect(),
    )
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
            ("task-assigned", "mention"),
            ("research-share", "mention"),
            ("dm", "dm"),
            ("agent-outreach", "dm"),
            // A reply an agent filed in your thread — yours, like a DM.
            ("agent-reply", "dm"),
            ("agent-problem", "agent_blocked"),
            ("workbench-repo-request", "agent_blocked"),
            ("research", "work_complete"),
            ("task-status", "work_complete"),
            // An access request's outcome — news, not a door. Explicit so the
            // quiet bucket is a decision here, not the fall-through's default.
            ("board_access", "work_complete"),
            // Wave 1 filed it through the fall-through; the table now says so.
            ("agent_google_principal", "work_complete"),
            // The MCP health pass's ok → down notification: in-app, like the
            // capability gaps it is one of.
            ("mcp_server_down", "gap_reported"),
            // Every class id is its own kind.
            ("judge_escalation", "judge_escalation"),
            ("gap_reported", "gap_reported"),
            ("approval_pending", "approval_pending"),
            ("workchain_turn", "workchain_turn"),
            ("workchain_paused", "workchain_paused"),
            ("agent_blocked", "agent_blocked"),
            ("work_complete", "work_complete"),
            // The two new classes' kinds (no producer files them yet).
            ("task-comment", "participating"),
            ("thread-reply", "participating"),
            ("channel-message", "channel_activity"),
        ] {
            assert_eq!(notify_class_of(kind), class, "kind {kind}");
        }
        // An unknown kind lands in the quiet bucket, never the mail-defaults.
        assert_eq!(notify_class_of("brand-new-kind"), "work_complete");
        assert_eq!(kind_reason("brand-new-kind"), None);
        assert_eq!(category_of("brand-new-kind"), Category::Following);
    }

    // Every KIND_CLASS row names a registered class, and every original class
    // is registered: a typo in either is a row that routes nowhere.
    #[test]
    fn the_table_and_the_registry_agree() {
        for (kind, class, reason) in KIND_CLASS {
            assert!(class_def(class).is_some(), "{kind} → {class}");
            assert!(!reason.is_empty(), "{kind}");
        }
        for id in ORIGINAL_CLASSES {
            assert!(class_def(id).is_some(), "{id}");
        }
        assert_eq!(
            NOTIFY_CLASSES.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            ORIGINAL_CLASSES.to_vec()
        );
        let mut kinds: Vec<&str> = KIND_CLASS.iter().map(|(k, _, _)| *k).collect();
        kinds.sort();
        let n = kinds.len();
        kinds.dedup();
        assert_eq!(kinds.len(), n, "a kind is listed twice");
    }

    // Phase 0 changes no delivery: each original class's registry defaults
    // ARE its wave-1 fallback route (`both` = desktop+push+email, `in_app` =
    // desktop+push), the shape `legacy_route` derives back.
    #[test]
    fn the_defaults_are_wave_ones_fallbacks() {
        for (id, fallback) in NOTIFY_CLASSES {
            let d = class_def(id).unwrap().defaults;
            assert!(d.desktop && d.push, "{id}");
            assert_eq!(d.email, *fallback == "both", "{id}");
        }
    }

    #[test]
    fn for_you_is_needs_you_plus_direct() {
        let k = for_you_kinds();
        for kind in [
            "approval_pending",
            "agent-problem",
            "workchain_turn",
            "judge_escalation",
            "mention",
            "kb-comment",
            "dm",
            "agent-reply",
            "task-comment",
            "agent_blocked",
        ] {
            assert!(k.contains(&kind), "{kind}");
        }
        for kind in [
            "research",
            "task-status",
            "channel-message",
            "mcp_server_down",
            "gap_reported",
        ] {
            assert!(!k.contains(&kind), "{kind}");
        }
    }

    #[test]
    fn the_wire_hides_admin_classes_from_members() {
        let admin = classes_wire(true);
        let member = classes_wire(false);
        let ids = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(ids(&admin).len(), CLASSES.len());
        assert!(!ids(&member).contains(&"agent_blocked".to_string()));
        assert!(!ids(&member).contains(&"gap_reported".to_string()));
        let mention = &admin[4];
        assert_eq!(mention["id"], "mention");
        assert_eq!(mention["category"], "direct");
        assert_eq!(
            mention["defaults"],
            json!({ "desktop": true, "push": true, "email": true })
        );
        assert_eq!(
            mention["kinds"][0],
            json!({ "id": "mention", "reason": "Mentioned you" })
        );
        // The key order is the wire contract.
        assert_eq!(
            mention
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            [
                "id",
                "label",
                "blurb",
                "category",
                "defaults",
                "breaksThrough",
                "adminOnly",
                "emailDelayMin",
                "kinds"
            ]
        );
    }
}

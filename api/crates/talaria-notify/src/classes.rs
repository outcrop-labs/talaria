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
    // An agent design finished (or failed) while its modal was closed.
    ("agent-design", "work_complete", "Agent design ready"),
    ("task-status", "work_complete", "Status changed"),
    // An access request's outcome — the request was yours, but the
    // notification is news, not a door.
    ("board_access", "work_complete", "Access decided"),
    // A Google principal for an agent was provisioned: an agent now reads
    // your mail, calendar and Drive. A security notice about YOUR account,
    // never the quiet outcome bucket the fall-through used to put it in
    // (SE-14).
    ("agent_google_principal", "account", "An agent acts as you"),
    ("workchain_paused", "workchain_paused", "Workchain paused"),
    // A channel at level `all` (W2-P files these).
    ("channel-message", "channel_activity", "New message"),
    // A capability agents lost: an MCP server the health pass saw go from ok
    // to down. In-app by default — an admin fixes it from the MCP page.
    ("gap_reported", "gap_reported", "Capability gap"),
    ("mcp_server_down", "gap_reported", "MCP server down"),
    // The agent image's stable channel stopped a rollout: the canary or an
    // agent failed on a new Hermes image, which is marked bad and never
    // rolled out again, and the agents stay on the last known good. The
    // workspace's own health, for admins, like a server down.
    (
        "hermes_update_failed",
        "gap_reported",
        "Agent image update failed",
    ),
    // The channel cannot do its job: its checks have failed for days, a
    // rollout has been open for a week, or its state row is unreadable.
    (
        "hermes_channel_stuck",
        "gap_reported",
        "Agent image updates stuck",
    ),
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
    /// What a person who never touched the matrix gets: wave 1's effective
    /// answer (push whenever the route kept an in-app copy, mail when the
    /// fallback route was `both`), except Following, which is quiet — the
    /// row lands in the feed and nothing banners, pushes or mails (EB-05,
    /// O-9) — but for `channel_activity`, which a person opts into by setting
    /// a channel to All. A stored legacy route or channel still wins
    /// (`channels_of`).
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
        id: "account",
        label: "Your account",
        blurb: "Changes to what can act as you, like an agent given your Google account.",
        category: Category::Direct,
        defaults: ch(true, true, true),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    // Following: news, not a door. Quiet by default (EB-05): the row lands in
    // the feed; a person who wants the banners turns them on.
    ClassDef {
        id: "work_complete",
        label: "Work finished",
        blurb: "A ticket moved, or research or an agent design you asked for is ready.",
        category: Category::Following,
        defaults: ch(false, false, false),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    ClassDef {
        id: "workchain_paused",
        label: "Workchains paused",
        blurb: "A workchain you run stopped and is waiting.",
        category: Category::Following,
        defaults: ch(false, false, false),
        breaks_through: false,
        admin_only: false,
        email_delay_min: 0,
    },
    // The one Following class that alerts by default: its rows exist only for
    // a channel the person set to All themselves (a channel starts at
    // Mentions), so choosing All IS the opt-in, and they get what every chat
    // app gives "All new messages" — the banner, the push and the missed-
    // activity mail a direct message gets. It stays Following: news about a
    // channel, in the All tab, off the For-you badge.
    ClassDef {
        id: "channel_activity",
        label: "Channel activity",
        blurb: "New messages in channels you set to notify on every message.",
        category: Category::Following,
        defaults: ch(true, true, true),
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
    class_def(kind).map(|c| c.id).unwrap_or(FALLBACK_CLASS)
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

/// The class every kind the registry does not name is filed under
/// (`notify_class_of`): the quiet bucket.
pub const FALLBACK_CLASS: &str = "work_complete";

/// The kinds that land in `class`: its `KIND_CLASS` kinds, then the class id
/// itself (a writer may name a class as its kind). What a `class=` filter
/// matches with `kind = any($1)` — plus, for `FALLBACK_CLASS` only, every kind
/// outside `known_kinds()`, which `notify_class_of` files there too.
pub fn kinds_of(class: &str) -> Vec<&'static str> {
    let Some(def) = class_def(class) else {
        return Vec::new();
    };
    let mut out: Vec<&'static str> = KIND_CLASS
        .iter()
        .filter(|(_, c, _)| *c == def.id)
        .map(|(kind, _, _)| *kind)
        .collect();
    if !out.contains(&def.id) {
        out.push(def.id);
    }
    out
}

/// The kinds of every class in `category` (`kinds_of`, class by class). A
/// `category=following` filter must also take every kind outside
/// `known_kinds()`: the fallback class is a Following one.
pub fn kinds_of_category(category: Category) -> Vec<&'static str> {
    CLASSES
        .iter()
        .filter(|c| c.category == category)
        .flat_map(|c| kinds_of(c.id))
        .collect()
}

/// Every kind the registry names: each `KIND_CLASS` kind and each class id. A
/// row whose kind is outside it is filed under `FALLBACK_CLASS`, so a SQL
/// filter that includes that class reads `kind = any(kinds_of(..)) or kind
/// <> all(known_kinds())`.
pub fn known_kinds() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = KIND_CLASS.iter().map(|(kind, _, _)| *kind).collect();
    for c in CLASSES {
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
            ("agent-design", "work_complete"),
            ("task-status", "work_complete"),
            // An access request's outcome — news, not a door. Explicit so the
            // quiet bucket is a decision here, not the fall-through's default.
            ("board_access", "work_complete"),
            // A security notice about your own account (SE-14), never the
            // quiet outcome bucket wave 1's fall-through put it in.
            ("agent_google_principal", "account"),
            // The MCP health pass's ok → down notification: in-app, like the
            // capability gaps it is one of.
            ("mcp_server_down", "gap_reported"),
            // The stable channel's stopped rollout: the workspace's health.
            ("hermes_update_failed", "gap_reported"),
            ("hermes_channel_stuck", "gap_reported"),
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

    // Every original class's default mail is its wave-1 fallback route
    // (`both` mails, `in_app` does not), so the digest's derived answer and
    // the legacy table never move. The screens are wave 1's (desktop and
    // push) everywhere but Following, which is quiet (EB-05) — all of it but
    // the channel a person set to All.
    #[test]
    fn the_defaults_are_wave_ones_but_following_is_quiet() {
        for (id, fallback) in NOTIFY_CLASSES {
            let c = class_def(id).unwrap();
            let d = c.defaults;
            assert_eq!(d.email, *fallback == "both", "{id}");
            if c.category == Category::Following {
                assert!(!d.desktop && !d.push && !d.email, "{id} is quiet");
            } else {
                assert!(d.desktop && d.push, "{id}");
            }
        }
        for c in CLASSES
            .iter()
            .filter(|c| c.category == Category::Following && c.id != "channel_activity")
        {
            assert_eq!(
                (c.defaults.desktop, c.defaults.push, c.defaults.email),
                (false, false, false),
                "{}",
                c.id
            );
        }
        // A channel the person set to All alerts as a direct message does:
        // they opted in by choosing All. Still news (Following): the All tab,
        // not the For-you badge.
        let channel = class_def("channel_activity").unwrap();
        assert_eq!(channel.defaults, class_def("dm").unwrap().defaults);
        assert_eq!(channel.category, Category::Following);
        assert!(!channel.breaks_through && !channel.admin_only);
        assert!(!ORIGINAL_CLASSES.contains(&"channel_activity"), "C27");
        let account = class_def("account").unwrap();
        assert_eq!(account.category, Category::Direct);
        assert!(account.defaults.desktop && account.defaults.push && account.defaults.email);
        assert!(!account.breaks_through && !account.admin_only);
    }

    // kinds_of / kinds_of_category / known_kinds agree with notify_class_of:
    // every named kind is in exactly its own class's list, the lists cover
    // the registry, and an unknown kind is outside known_kinds (so a filter
    // on the fallback class takes it by exclusion).
    #[test]
    fn kinds_of_partitions_the_named_kinds() {
        let known = known_kinds();
        let mut union: Vec<&str> = Vec::new();
        for c in CLASSES {
            for kind in kinds_of(c.id) {
                assert_eq!(notify_class_of(kind), c.id, "{kind} listed under {}", c.id);
                assert!(!union.contains(&kind), "{kind} listed twice");
                union.push(kind);
            }
        }
        let (mut a, mut b) = (union.clone(), known.clone());
        a.sort();
        b.sort();
        assert_eq!(a, b);
        assert!(kinds_of("mention").starts_with(&["mention", "kb-comment"]));
        assert_eq!(
            kinds_of("participating"),
            ["task-comment", "thread-reply", "participating"]
        );
        assert!(kinds_of("no-such-class").is_empty());
        assert!(!known.contains(&"brand-new-kind"));
        assert_eq!(notify_class_of("brand-new-kind"), FALLBACK_CLASS);
        let mut fy = for_you_kinds();
        let mut cat: Vec<&str> = kinds_of_category(Category::NeedsYou);
        cat.extend(kinds_of_category(Category::Direct));
        fy.sort();
        cat.sort();
        assert_eq!(fy, cat, "For you = needs_you + direct, kind for kind");
        assert!(kinds_of_category(Category::Following).contains(&"task-status"));
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

// What a producer can say about a notification beyond its content — the
// contract's shared types (docs/notifications/contract.md, I-24). FROZEN for
// wave 2: every stream reads these, none edits them; a miss is said in the PR
// that finds it and fixed after.
//
// `add_notification` files with `NotifyMeta::default()`, which is exactly
// wave 1's behaviour: no key, no window, no entity, no actor, alert by the
// person's matrix.

/// How a producer files one row (`add_notification_with`).
#[derive(Clone, Debug, Default)]
pub struct NotifyMeta {
    /// Land in-app (and count) but never toast, banner, push or mail: a
    /// producer's mute decision (C16). The row's `alert` is `feed`.
    pub quiet: bool,
    /// One OPEN row per (person, key) — see `group_key::*`. A write with a key
    /// coalesces into the open row instead of filing a second one.
    pub group_key: Option<String>,
    /// Past this long since the open row last changed, it keeps its content
    /// and loses its key, and a fresh row is filed. `None`: no window.
    pub group_window_secs: Option<u32>,
    /// What a coalesce does to the open row (ignored without a key).
    pub coalesce: Coalesce,
    /// Before inserting, adopt an open, unkeyed row of the same kind and href
    /// (a row filed before the contract) into the group. The id stays, so
    /// whatever is keyed by it (an inbox-focus snooze) stays with it.
    pub adopt_legacy: bool,
    /// What the row is about, so it can be cleared when that thing resolves.
    pub entity: Option<NotifyEntity>,
    /// Who caused it.
    pub actor: Option<NotifyActor>,
    /// Break through DND: a decision a person is blocking (W2-E enforces).
    pub force_alert: bool,
    /// Never web-push this row.
    pub skip_push: bool,
}

/// What a coalesce does to an open row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Coalesce {
    /// `created_at` moves to now, so every client reads a re-alert (a known
    /// id whose createdAt moved later) and the row rises in the list.
    #[default]
    Realert,
    /// Count and content update; `created_at` stays; no client alerts, and
    /// nothing is pushed or mailed.
    Quiet,
}

/// The thing a row is about: an approval, a task, an MCP server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyEntity {
    /// One of `entity::*`.
    pub kind: &'static str,
    pub id: String,
}

impl NotifyEntity {
    pub fn new(kind: &'static str, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

/// Who caused a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind {
    User,
    Agent,
    System,
}

impl ActorKind {
    /// The stored and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ActorKind::User => "user",
            ActorKind::Agent => "agent",
            ActorKind::System => "system",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyActor {
    pub kind: ActorKind,
    /// A user's uuid, an agent's model id, or a system source's name.
    pub id: String,
    /// What to call them when they are not a user row (an agent's label). A
    /// user actor's name is read from `users.name` at read time.
    pub label: String,
}

/// How a client presents an arrival (I-15). `None` in storage is a legacy
/// row, which reads as `Banner`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Alert {
    /// Toast in a looked-at tab, an OS banner otherwise.
    Banner,
    /// Recorded and counted; never toasted or bannered.
    Feed,
}

impl Alert {
    pub fn as_str(self) -> &'static str {
        match self {
            Alert::Banner => "banner",
            Alert::Feed => "feed",
        }
    }
}

/// What one write did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filed {
    pub id: String,
    /// The person's notify seq this write took (I-10).
    pub seq: i64,
    /// How many writes the row now carries (1 for a fresh row).
    pub count: i32,
    /// A fresh row (true) or a coalesce into an open one (false).
    pub inserted: bool,
    /// The row's createdAt as stored, epoch ms.
    pub created_ms: i64,
    pub alert: Alert,
}

/// The group-key conventions (I-24). Producers call these, never `format!()`,
/// so two producers that mean the same group can never spell it two ways.
pub mod group_key {
    /// One open row per DM conversation.
    pub fn dm(channel_id: &str) -> String {
        format!("dm:{channel_id}")
    }
    /// One open row per agent thread.
    pub fn thread(conversation_id: &str) -> String {
        format!("thread:{conversation_id}")
    }
    /// A ticket's status moves.
    pub fn task_status(task_id: &str) -> String {
        format!("task:{task_id}:status")
    }
    /// A ticket's comments.
    pub fn task_comments(task_id: &str) -> String {
        format!("task:{task_id}:comments")
    }
    /// A channel's messages, at level `all`.
    pub fn channel_activity(channel_id: &str) -> String {
        format!("channel:{channel_id}:activity")
    }
    /// Replies to one channel message.
    pub fn channel_thread(root_message_id: &str) -> String {
        format!("reply:{root_message_id}")
    }
    /// One MCP server down.
    pub fn mcp_down(server_id: &str) -> String {
        format!("mcp:{server_id}:down")
    }
    /// One approval waiting.
    pub fn approval(approval_id: &str) -> String {
        format!("approval:{approval_id}")
    }
    /// An agent's outreach on one day (`yyyy-mm-dd`).
    pub fn outreach(agent: &str, day: &str) -> String {
        format!("outreach:{agent}:{day}")
    }
}

/// The entity kinds a row can be about (the stored `entity_type`).
pub mod entity {
    pub const TASK: &str = "task";
    pub const CHANNEL: &str = "channel";
    pub const CONVERSATION: &str = "conversation";
    pub const APPROVAL: &str = "approval";
    pub const MCP_SERVER: &str = "mcp_server";
    pub const PLAN: &str = "plan";
    pub const RESEARCH_RUN: &str = "research_run";
    pub const KB_DOC: &str = "kb_doc";
}

#[cfg(test)]
mod tests {
    use super::*;

    // The spellings are the contract: the TS twin (ui/src/lib/notify-wire.ts
    // GROUP/ENTITY) and every producer's rows depend on them.
    #[test]
    fn group_keys_and_entities_are_pinned() {
        assert_eq!(group_key::dm("c1"), "dm:c1");
        assert_eq!(group_key::thread("v1"), "thread:v1");
        assert_eq!(group_key::task_status("t1"), "task:t1:status");
        assert_eq!(group_key::task_comments("t1"), "task:t1:comments");
        assert_eq!(group_key::channel_activity("c1"), "channel:c1:activity");
        assert_eq!(group_key::channel_thread("m1"), "reply:m1");
        assert_eq!(group_key::mcp_down("s1"), "mcp:s1:down");
        assert_eq!(group_key::approval("a1"), "approval:a1");
        assert_eq!(
            group_key::outreach("margaret", "2026-10-31"),
            "outreach:margaret:2026-10-31"
        );
        assert_eq!(
            [
                entity::TASK,
                entity::CHANNEL,
                entity::CONVERSATION,
                entity::APPROVAL,
                entity::MCP_SERVER,
                entity::PLAN,
                entity::RESEARCH_RUN,
                entity::KB_DOC
            ],
            [
                "task",
                "channel",
                "conversation",
                "approval",
                "mcp_server",
                "plan",
                "research_run",
                "kb_doc"
            ]
        );
        assert_eq!(serde_json::to_string(&Alert::Feed).unwrap(), "\"feed\"");
        assert_eq!(Alert::Banner.as_str(), "banner");
        assert_eq!(ActorKind::Agent.as_str(), "agent");
        let m = NotifyMeta::default();
        assert!(!m.quiet && m.group_key.is_none() && m.coalesce == Coalesce::Realert);
    }
}

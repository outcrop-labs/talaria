// The platform facts an app's `event` trigger may name, and the one door they
// leave the engines through.
//
// An engine that already announces a change to its own audience — a ticket
// created, a ticket moved to another column — calls `emit` beside that
// fan-out, and whatever the composition root installed as the sink
// (talaria-app-triggers, wired in talaria-jobs) decides which apps hear it.
// A leaf with no talaria deps, so an engine can emit without reaching up into
// the apps plane (docs/RUST-MIGRATION.md, "Dependencies point DOWN only").
//
// AN EVENT CARRIES IDS, NEVER CONTENT. An app's trigger runs as the app, with
// no user behind it, so nobody's read ACL stands between a payload and the
// app's code: a ticket's title or description here would be a read path
// around every board's membership. Ids, column keys and closed flags say
// WHAT changed; an app that needs more asks the platform as a person later,
// through the ordinary routes (the rule talaria-realtime keeps for its
// fan-out, for the same reason).
//
// THE SET IS SMALL ON PURPOSE. Two events that already had an internal path
// (the work dispatch and the notifications ride the same two writes), named
// in `EVENT_NAMES`, which the manifest validator holds every `event` trigger
// to. A new one is a new variant, a new name and an `emit` call at the
// write that already announces it.

use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};

/// Every event an app may subscribe to, by the name `talaria.json` uses. The
/// TS host's manifest twin keeps the same list, pinned by the shared fixture
/// (api/tests/it/fixtures/app-manifest.json `events`).
pub const EVENT_NAMES: [&str; 2] = ["ticket.created", "ticket.moved"];

/// Whether `name` is one of `EVENT_NAMES`.
pub fn is_event(name: &str) -> bool {
    EVENT_NAMES.contains(&name)
}

/// One platform fact. Each variant is ids and closed values only (see the
/// file header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformEvent {
    /// A ticket was created, on any board.
    TicketCreated { ticket_id: String, board_id: String },
    /// A ticket moved from one column to another — however it moved: a
    /// person's drag, promotion by assignment, an agent's status write, a
    /// review's sign-off or rejection. `closed` says the column it landed in
    /// is terminal on its board (a done column, or failed/cancelled).
    TicketMoved {
        ticket_id: String,
        board_id: String,
        from: String,
        to: String,
        closed: bool,
    },
}

impl PlatformEvent {
    /// The event's name, as a trigger names it.
    pub fn name(&self) -> &'static str {
        match self {
            PlatformEvent::TicketCreated { .. } => "ticket.created",
            PlatformEvent::TicketMoved { .. } => "ticket.moved",
        }
    }

    /// What the trigger's handler receives besides the name.
    pub fn payload(&self) -> Value {
        match self {
            PlatformEvent::TicketCreated {
                ticket_id,
                board_id,
            } => json!({ "ticketId": ticket_id, "boardId": board_id }),
            PlatformEvent::TicketMoved {
                ticket_id,
                board_id,
                from,
                to,
                closed,
            } => json!({
                "ticketId": ticket_id,
                "boardId": board_id,
                "from": from,
                "to": to,
                "closed": closed,
            }),
        }
    }
}

/// Where emitted events go. It must not block: it is called inline in the
/// engine's write path, so a sink that has work to do spawns it.
pub type Sink = Arc<dyn Fn(PlatformEvent) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();

/// Install the sink, once, at boot. False when one was already installed —
/// the first stays.
pub fn set_sink(sink: Sink) -> bool {
    SINK.set(sink).is_ok()
}

/// Announce `event`. With no sink installed (a test, a tool binary) it goes
/// nowhere, which is the honest answer: nothing is listening.
pub fn emit(event: PlatformEvent) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_is_named_in_the_list() {
        let all = [
            PlatformEvent::TicketCreated {
                ticket_id: "t".into(),
                board_id: "b".into(),
            },
            PlatformEvent::TicketMoved {
                ticket_id: "t".into(),
                board_id: "b".into(),
                from: "todo".into(),
                to: "done".into(),
                closed: true,
            },
        ];
        let names: Vec<&str> = all.iter().map(PlatformEvent::name).collect();
        assert_eq!(names, EVENT_NAMES);
        assert!(names.iter().all(|n| is_event(n)));
        assert!(!is_event("ticket.deleted"));
    }

    #[test]
    fn a_payload_carries_ids_and_columns_only() {
        let moved = PlatformEvent::TicketMoved {
            ticket_id: "t1".into(),
            board_id: "b1".into(),
            from: "todo".into(),
            to: "done".into(),
            closed: true,
        };
        assert_eq!(
            moved.payload(),
            json!({"ticketId": "t1", "boardId": "b1", "from": "todo", "to": "done", "closed": true})
        );
        let created = PlatformEvent::TicketCreated {
            ticket_id: "t1".into(),
            board_id: "b1".into(),
        };
        assert_eq!(
            created.payload(),
            json!({"ticketId": "t1", "boardId": "b1"})
        );
    }
}

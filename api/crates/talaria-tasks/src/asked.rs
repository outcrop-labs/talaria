// A PERSON'S ASK, PROVEN — the one way an agent takes a ticket out of review.
//
// A review column is the human sign-off queue, and an agent never empties it
// on its own (`agent_safe_patch`'s review clause, `pick_move_status`'s review
// stop). What changes here is the case a teammate ASKS for: "ABC-12 needs
// revisions, send it back to In progress." The agent's call then carries the
// thread it was asked in (`conversationId`, the toolkit's name for "the
// thread you are answering"), and the server proves the ask before the move:
//
//   caller     the agent's own `tak_` key. A legacy shared-key caller only
//              asserts which agent it is, so it can never cite anybody.
//   live turn  this agent is replying in that thread NOW (talaria-chips
//              `live_turn`, streaming rows only) — so a cron, a work session,
//              a finished chat or another agent's thread cannot be cited.
//   the asker  the author of a human message that live turn answers, and
//              nobody else: never a bystander in a busy room, never words the
//              agent drafted that the person only clicked (`drafted` rows).
//              A channel reply answers the one message stamped on its row;
//              a turn answering several people, or a call that cannot say
//              which of the agent's replies it comes from, cites nobody.
//   the ticket the message names it (its ref, an old ref, its id or link), or
//              the turn is in the ticket's own discussion room.
//   authority  the asker can EDIT the board (both boards for a board move) —
//              an agent never does more for someone than they could do.
//   once       one exit per (ticket, message), claimed under a unique key
//              (`task_asked_exits`), so a message is not a standing grant.
//
// Terminal moves (done / failed / cancelled) stay a person's own, asked or
// not: the callers refuse them before they get here. A failed proof refuses
// with the reason and points at the approval card the refused call leaves in
// the thread (talaria-chips `chips_from_tool`), which a board editor approves
// and which then runs the move as that person.

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_boards::{board_role, can_edit};
use talaria_chips::{TurnLookup, TurnMessage, TurnPlace, live_turn};
use talaria_tasks_types::{AskClaim, Task};

/// The refusal when an agent tries to leave review with no ask at all.
pub const NO_ASK: &str = "agents cannot take a ticket out of review on their own — a person signs it off. If the person you are answering asked for this move, call again with conversationId set to your thread's id (your prompt's thread line carries it); otherwise a person who can edit the board approves it from the card this refusal leaves in your thread";

/// The refusal for a closed destination when the agent cited an ask.
pub const NEVER_CLOSES: &str = "agents cannot close a ticket, even when a person asks — done, failed and cancelled are that person's own move";

/// Who asked, and where: what the activity line, the audit row and the
/// one-exit claim record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asker {
    pub user_id: String,
    /// Their email, else their name — how the activity line names them.
    pub label: String,
    pub message_id: String,
    pub thread_id: String,
    pub place: AskPlace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskPlace {
    Chat,
    /// The ticket's own discussion room.
    TicketRoom,
    /// Any other channel, by name (empty for an unnamed group DM).
    Channel(String),
}

impl Asker {
    /// The note the status line carries: "moved to in_progress (asked by
    /// ada@example.com in chat)". The line's actor is the agent, so the
    /// record names both.
    pub fn note(&self) -> String {
        let place = match &self.place {
            AskPlace::Chat => "in chat".to_string(),
            AskPlace::TicketRoom => "in the ticket's discussion".to_string(),
            AskPlace::Channel(name) if name.trim().is_empty() => "in a direct message".into(),
            AskPlace::Channel(name) => format!("in #{name}"),
        };
        format!("asked by {} {place}", self.label)
    }

    /// The audit row's `askedBy` / `askedIn` pair.
    pub fn audit(&self) -> Value {
        json!({
            "askedBy": {
                "userId": self.user_id,
                "label": self.label,
                "messageId": self.message_id,
            },
            "askedIn": {
                "kind": match self.place {
                    AskPlace::Chat => "conversation",
                    AskPlace::TicketRoom | AskPlace::Channel(_) => "channel",
                },
                "id": self.thread_id,
            },
        })
    }
}

/// Why an ask did not prove. Every sentence says what would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unproven {
    SharedKey,
    NotYours,
    NotLive,
    /// Two or more of the agent's replies are streaming in the thread.
    SeveralTurns,
    /// The messages the turn answers come from more than one person.
    SeveralPeople,
    NoPerson,
    Drafted,
    NotNamed,
    CannotEdit(String),
    Spent,
}

impl Unproven {
    pub fn sentence(&self) -> String {
        match self {
            Unproven::SharedKey => {
                "a shared agent key cannot carry a person's ask — call with your own agent key"
                    .into()
            }
            Unproven::NotYours => "conversationId names no thread of yours — pass the id your prompt's thread line gives".into(),
            Unproven::NotLive => "you are not replying in that thread right now — the ask has to be the message you are answering".into(),
            Unproven::SeveralTurns => "you are writing more than one reply in that thread right now, so Talaria cannot tell which message you are answering — the ask has to come while one reply is open".into(),
            Unproven::SeveralPeople => "the messages you are answering come from more than one person, so Talaria cannot tell which of them asked — the ask has to be the only person's word in the turn".into(),
            Unproven::NoPerson => "the message you are answering has no person behind it that Talaria can name".into(),
            Unproven::Drafted => "the message you are answering is words you wrote that the person only clicked, which is not their ask".into(),
            Unproven::NotNamed => "the message you are answering does not name this ticket — ask the person to name it by its ref or link".into(),
            Unproven::CannotEdit(label) => format!(
                "{label} cannot edit every board this move touches, so they could not make the move themselves either"
            ),
            Unproven::Spent => "that message already took this ticket out of review once — another move needs a new ask".into(),
        }
    }

    /// The whole refusal an agent reads: the rule, why the ask did not
    /// prove, and the person's way through.
    pub fn refusal(&self) -> String {
        format!(
            "agents cannot take a ticket out of review on their own — {}. A person who can edit the board approves the move from the card this refusal leaves in your thread",
            self.sentence()
        )
    }
}

/// Does `text` name the ticket: one of its refs as a whole word, any case
/// (`ABC-12` never matches `ABC-120` or `XABC-12`), or its id — which also
/// covers a pasted `/boards/<board>/<id>` link.
pub fn names_ticket(text: &str, refs: &[String], id: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    // ASCII lowering keeps byte offsets, so the boundary checks below index
    // the same string they searched.
    let hay = text.to_ascii_lowercase();
    let id = id.trim().to_ascii_lowercase();
    if !id.is_empty() && hay.contains(&id) {
        return true;
    }
    refs.iter()
        .map(|r| r.trim().to_ascii_lowercase())
        .filter(|r| !r.is_empty())
        .any(|r| whole_word(&hay, &r))
}

fn whole_word(hay: &str, needle: &str) -> bool {
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(at) = hay[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
        while !hay.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

/// The message that is the ask, out of the ones the live turn answers
/// (oldest first): the newest one a person wrote — not drafted, with a person
/// the row proves — that names the ticket, or any such one in the ticket's own
/// room. A turn answering more than one person cites nobody: whichever of
/// them the agent is acting for, a naming row by another cannot stand in for
/// them (rows queued behind a reply included). Pure; the authority check is
/// the caller's.
pub fn choose_ask<'a>(
    messages: &'a [TurnMessage],
    in_room: bool,
    refs: &[String],
    id: &str,
) -> Result<&'a TurnMessage, Unproven> {
    // Every row counts here, clicked or unprovable ones too: a click is
    // still that person's input to the turn.
    if let Some(first) = messages.first()
        && messages.iter().any(|m| m.user_id != first.user_id)
    {
        return Err(Unproven::SeveralPeople);
    }
    let written: Vec<&TurnMessage> = messages.iter().filter(|m| !m.drafted).collect();
    if written.is_empty() {
        return Err(if messages.is_empty() {
            Unproven::NoPerson
        } else {
            Unproven::Drafted
        });
    }
    let proven: Vec<&TurnMessage> = written
        .into_iter()
        .filter(|m| m.user_id.is_some())
        .collect();
    if proven.is_empty() {
        return Err(Unproven::NoPerson);
    }
    proven
        .into_iter()
        .rev()
        .find(|m| in_room || names_ticket(&m.content, refs, id))
        .ok_or(Unproven::NotNamed)
}

/// Every ref the ticket answers to: its live one and the ones it left behind
/// on boards it moved from (`task_ref_aliases`).
async fn ticket_refs(pg: &PgPool, ticket: &Task) -> Result<Vec<String>, sqlx::Error> {
    let mut refs: Vec<String> = ticket.ticket_ref.iter().cloned().collect();
    let aliases: Vec<(String,)> = sqlx::query_as(
        "select coalesce(b.ticket_prefix, 'TASK') || '-' || a.ticket_no \
         from task_ref_aliases a join boards b on b.id = a.board_id \
         where a.task_id = $1::uuid",
    )
    .bind(&ticket.id)
    .fetch_all(pg)
    .await?;
    refs.extend(aliases.into_iter().map(|(r,)| r));
    Ok(refs)
}

/// The caller half, before any read: a legacy shared-key caller only asserts
/// which agent it is, so whatever thread it names, it cites nobody.
fn caller_refusal(claim: &AskClaim) -> Option<Unproven> {
    (!claim.proven_caller).then_some(Unproven::SharedKey)
}

/// Prove `claim` for `agent` moving `ticket` out of review. `edit_boards` are
/// the boards the asker must be able to edit: the ticket's, or both ends of a
/// board move — the rule a person's own move obeys.
pub async fn person_asked(
    pg: &PgPool,
    agent: &str,
    ticket: &Task,
    edit_boards: &[&str],
    claim: &AskClaim,
) -> Result<Result<Asker, Unproven>, sqlx::Error> {
    if let Some(why) = caller_refusal(claim) {
        return Ok(Err(why));
    }
    let turn = match live_turn(pg, agent, &claim.thread_id).await? {
        TurnLookup::NotYours => return Ok(Err(Unproven::NotYours)),
        TurnLookup::NotLive => return Ok(Err(Unproven::NotLive)),
        TurnLookup::Several => return Ok(Err(Unproven::SeveralTurns)),
        TurnLookup::Live(turn) => turn,
    };
    let in_room =
        matches!(&turn.place, TurnPlace::Channel { task_id: Some(t), .. } if *t == ticket.id);
    let refs = ticket_refs(pg, ticket).await?;
    let row = match choose_ask(&turn.messages, in_room, &refs, &ticket.id) {
        Ok(row) => row,
        Err(why) => return Ok(Err(why)),
    };
    let Some(user_id) = row.user_id.clone() else {
        return Ok(Err(Unproven::NoPerson));
    };
    for board in edit_boards {
        if !can_edit(board_role(pg, &user_id, board).await?.as_deref()) {
            return Ok(Err(Unproven::CannotEdit(row.label.clone())));
        }
    }
    let place = match turn.place {
        TurnPlace::Conversation => AskPlace::Chat,
        TurnPlace::Channel { .. } if in_room => AskPlace::TicketRoom,
        TurnPlace::Channel { name, .. } => AskPlace::Channel(name),
    };
    Ok(Ok(Asker {
        user_id,
        label: row.label.clone(),
        message_id: row.id.clone(),
        thread_id: turn.thread_id,
        place,
    }))
}

/// Take the one exit this message allows for this ticket. False when it was
/// already taken — a second call citing the same message, concurrent or not.
pub async fn claim_exit<'e, E: sqlx::PgExecutor<'e>>(
    ex: E,
    task_id: &str,
    agent: &str,
    asker: &Asker,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "insert into task_asked_exits (task_id, message_id, agent_model, asked_by) \
         values ($1::uuid, $2::uuid, $3, $4::uuid) on conflict do nothing",
    )
    .bind(task_id)
    .bind(&asker.message_id)
    .bind(agent)
    .bind(&asker.user_id)
    .execute(ex)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Give the exit back when the write it was claimed for did not land.
pub async fn release_exit(pg: &PgPool, task_id: &str, asker: &Asker) {
    if let Err(e) = sqlx::query(
        "delete from task_asked_exits where task_id = $1::uuid and message_id = $2::uuid",
    )
    .bind(task_id)
    .bind(&asker.message_id)
    .execute(pg)
    .await
    {
        tracing::warn!("[tasks] asked exit on {task_id} not released: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, user: Option<&str>, content: &str, drafted: bool) -> TurnMessage {
        TurnMessage {
            id: id.into(),
            user_id: user.map(str::to_string),
            label: user.map(|u| format!("{u}@example.com")).unwrap_or_default(),
            content: content.into(),
            drafted,
        }
    }

    const ID: &str = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

    fn refs() -> Vec<String> {
        vec!["ABC-12".into(), "OLD-7".into()]
    }

    #[test]
    fn a_ref_names_the_ticket_as_a_whole_word_in_any_case() {
        assert!(names_ticket("ABC-12 needs revisions", &refs(), ID));
        assert!(names_ticket("send abc-12 back please", &refs(), ID));
        assert!(names_ticket("(ABC-12)", &refs(), ID));
        assert!(names_ticket("look at #ABC-12.", &refs(), ID));
    }

    #[test]
    fn a_longer_ref_is_not_this_one() {
        assert!(!names_ticket("ABC-120 needs revisions", &refs(), ID));
        assert!(!names_ticket("XABC-12 needs revisions", &refs(), ID));
        assert!(!names_ticket("ABC-12a", &refs(), ID));
        // ...but a second occurrence further on still counts.
        assert!(names_ticket("not ABC-120 but ABC-12", &refs(), ID));
    }

    #[test]
    fn an_old_ref_the_id_and_a_link_name_it() {
        assert!(names_ticket("old-7 should go back", &refs(), ID));
        assert!(names_ticket(&format!("ticket {ID}"), &refs(), ID));
        assert!(names_ticket(
            &format!("https://talaria.example/boards/b-1/{}", ID.to_uppercase()),
            &refs(),
            ID
        ));
    }

    #[test]
    fn nothing_names_it_from_nothing() {
        assert!(!names_ticket("", &refs(), ID));
        assert!(!names_ticket("   ", &refs(), ID));
        assert!(!names_ticket("move it back", &refs(), ID));
        assert!(!names_ticket("move it back", &[], ""));
        // Multi-byte text around a near miss never panics.
        assert!(!names_ticket("é ABC-1 — ünd", &refs(), ID));
    }

    #[test]
    fn in_the_room_a_written_message_need_not_name_it() {
        let m = [msg(
            "m1",
            Some("ada"),
            "send this back to in progress",
            false,
        )];
        assert_eq!(choose_ask(&m, true, &refs(), ID).unwrap().id, "m1");
        assert_eq!(choose_ask(&m, false, &refs(), ID), Err(Unproven::NotNamed));
    }

    #[test]
    fn outside_the_room_only_a_message_naming_it_counts() {
        let m = [
            msg("m1", Some("ada"), "ABC-12 needs revisions", false),
            msg("m2", Some("ada"), "also, thanks", false),
        ];
        assert_eq!(choose_ask(&m, false, &refs(), ID).unwrap().id, "m1");
    }

    #[test]
    fn the_newest_naming_message_is_the_ask() {
        let m = [
            msg("m1", Some("ada"), "ABC-12 looks off", false),
            msg("m2", Some("ada"), "move ABC-12 back", false),
        ];
        assert_eq!(choose_ask(&m, false, &refs(), ID).unwrap().id, "m2");
    }

    /// A turn answering two people cites neither: the agent may be acting on
    /// one person's words while another's row names the ticket — a viewer's
    /// "send it back" and an editor's "what's left on ABC-12?", say.
    #[test]
    fn a_turn_answering_more_than_one_person_cites_nobody() {
        let viewer_then_editor = [
            msg("m1", Some("vic"), "send it back to in progress", false),
            msg("m2", Some("eve"), "what's left on ABC-12?", false),
        ];
        for in_room in [false, true] {
            assert_eq!(
                choose_ask(&viewer_then_editor, in_room, &refs(), ID),
                Err(Unproven::SeveralPeople)
            );
        }
        // A click is still the clicker's input to the turn.
        let clicked = [
            msg("m1", Some("vic"), "Yes, send it back", true),
            msg("m2", Some("eve"), "ABC-12 back please", false),
        ];
        assert_eq!(
            choose_ask(&clicked, false, &refs(), ID),
            Err(Unproven::SeveralPeople)
        );
        // A row no person can be held to is another voice too.
        let unproven = [
            msg("m1", None, "ABC-12 back", false),
            msg("m2", Some("eve"), "ABC-12 back please", false),
        ];
        assert_eq!(
            choose_ask(&unproven, false, &refs(), ID),
            Err(Unproven::SeveralPeople)
        );
    }

    #[test]
    fn drafted_words_are_never_the_ask() {
        let m = [msg("m1", Some("ada"), "Yes, move ABC-12 back", true)];
        assert_eq!(choose_ask(&m, false, &refs(), ID), Err(Unproven::Drafted));
        assert_eq!(choose_ask(&m, true, &refs(), ID), Err(Unproven::Drafted));
        // A written message beside a clicked one still counts.
        let m = [
            msg("m1", Some("ada"), "ABC-12 should go back", false),
            msg("m2", Some("ada"), "Bullets", true),
        ];
        assert_eq!(choose_ask(&m, false, &refs(), ID).unwrap().id, "m1");
    }

    #[test]
    fn no_message_or_no_person_is_no_ask() {
        assert_eq!(choose_ask(&[], true, &refs(), ID), Err(Unproven::NoPerson));
        let m = [msg("m1", None, "ABC-12 back please", false)];
        assert_eq!(choose_ask(&m, false, &refs(), ID), Err(Unproven::NoPerson));
    }

    #[test]
    fn every_refusal_says_what_would_prove_it() {
        let fixes = [
            (Unproven::SharedKey, "your own agent key"),
            (Unproven::NotYours, "thread line"),
            (Unproven::NotLive, "message you are answering"),
            (Unproven::SeveralTurns, "one reply is open"),
            (Unproven::SeveralPeople, "only person's word"),
            (Unproven::NoPerson, "no person"),
            (Unproven::Drafted, "not their ask"),
            (Unproven::NotNamed, "ref or link"),
            (
                Unproven::CannotEdit("bo@example.com".into()),
                "bo@example.com",
            ),
            (Unproven::Spent, "new ask"),
        ];
        for (why, fix) in fixes {
            let line = why.refusal();
            assert!(line.contains(fix), "{why:?}: {line}");
            assert!(line.starts_with("agents cannot take a ticket out of review"));
            // The refused call leaves an approval card; the sentence says so.
            assert!(line.contains("card"), "{why:?}");
        }
        assert!(NO_ASK.contains("conversationId") && NO_ASK.contains("card"));
        assert!(NEVER_CLOSES.contains("even when a person asks"));
    }

    #[test]
    fn a_shared_key_caller_cites_nobody() {
        let claim = |proven_caller| AskClaim {
            thread_id: ID.into(),
            proven_caller,
        };
        assert_eq!(caller_refusal(&claim(false)), Some(Unproven::SharedKey));
        assert_eq!(caller_refusal(&claim(true)), None);
    }

    #[test]
    fn the_note_names_the_asker_and_the_place() {
        let mut a = Asker {
            user_id: "u-1".into(),
            label: "ada@example.com".into(),
            message_id: "m-1".into(),
            thread_id: "c-1".into(),
            place: AskPlace::Chat,
        };
        assert_eq!(a.note(), "asked by ada@example.com in chat");
        a.place = AskPlace::TicketRoom;
        assert_eq!(
            a.note(),
            "asked by ada@example.com in the ticket's discussion"
        );
        a.place = AskPlace::Channel("eng".into());
        assert_eq!(a.note(), "asked by ada@example.com in #eng");
        a.place = AskPlace::Channel(String::new());
        assert_eq!(a.note(), "asked by ada@example.com in a direct message");
        let audit = a.audit();
        assert_eq!(audit["askedBy"]["messageId"], "m-1");
        assert_eq!(audit["askedIn"]["kind"], "channel");
    }
}

// THE TICKET THREAD'S MODEL SURFACES. The chat door (routes/comms) owns the
// turn; this file owns what is ticket-specific about it: the head of the
// task a conversation is attached to, the context block that tells an agent
// which ticket's room it is speaking in, and the relevance gate that
// decides whether a human message there deserves an agent turn at all.
//
// THE BINDING THIS FILE SERVES (see ensure_task_conversation in tasks.rs):
// a ticket thread's agent_model is the task's first agent assignee. The
// binder alone would make the agent a roommate — one answer to every
// message in a room where most messages are people talking to each other —
// and the gate below is what makes a live binder safe to leave on an
// unassigned-but-bound thread: the column says WHO would answer, the gate
// says WHETHER.
//
// Both fail open, and for the same reason: nothing here may ever be the
// reason a human message goes unanswered. A gate that cannot run costs one
// slightly unnecessary reply; the alternative is a thread that looks
// delivered and isn't.

use serde_json::Value;
use serde_json::json;
use sqlx::PgPool;

use crate::body::truncate_utf16;
use crate::harness::defs::ticket_relevance::{TicketRelevanceInput, ticket_relevance_harness};
use crate::harness::run::{RunContext, run_harness};
use crate::state::AppState;

// ── The head ─────────────────────────────────────────────────────────────────

/// The ticket a thread is attached to, read once and reused by everything
/// below — the prompt block and the gate must agree on what the room is
/// about, which is the same as saying they must read the same row.
#[derive(Debug, Clone)]
pub struct TicketHead {
    /// The board's ref ("WEB-31") when the board numbers tickets; a board
    /// without numbering has none, and every consumer renders the title
    /// alone rather than inventing a ref.
    pub ticket_ref: Option<String>,
    pub title: String,
    pub status: String,
    pub description: Option<String>,
}

impl TicketHead {
    /// Ref and title on one line — the line the gate's input carries and
    /// the prompt block leads with, so the judge and the agent can never
    /// be looking at two different rooms.
    pub fn line(&self) -> String {
        match &self.ticket_ref {
            Some(r) => format!("{r} — {}", self.title),
            None => self.title.clone(),
        }
    }
}

/// Read the head of the task a conversation belongs to. None when the task
/// is gone (a deleted task leaves its conversation orphaned — on delete
/// set null — and nothing here needs to be the thing that notices).
pub async fn ticket_head(pg: &PgPool, task_id: &str) -> Option<TicketHead> {
    sqlx::query_as::<_, (Option<String>, String, String, Option<String>)>(
        "select case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') \
                 || '-' || t.ticket_no end, \
                t.title, t.status, t.description \
         from tasks t join boards b on b.id = t.board_id \
         where t.id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .map(|(ticket_ref, title, status, description)| TicketHead {
        ticket_ref,
        title,
        status,
        description,
    })
}

// ── The prompt ───────────────────────────────────────────────────────────────

/// The ticket-mode harness, prepended to every ticket-thread turn. Carries
/// what is true of the ROOM (multi-human, the agent is assigned to the
/// work, not the conversation) and the FIRST REACH: the board tools, in the
/// order the work actually uses them. The ticket's own identity rides in
/// the context block appended after this — `ticket_context_block` — the
/// same split the plan surface uses (constant mode, live routing).
pub const TICKET_MODE_PROMPT: &str = "This is a TICKET\u{2019}s discussion thread — the conversation attached to one task on a board. You are the agent assigned to that task; the room is everyone who can see the board, and most messages in it are people talking to each other. Speak as a teammate doing the work, not as a chat assistant: reply about the ticket\u{2019}s work, take direction from the thread, and stay out of conversations that are not yours.
The board tools are your first reach: get_ticket to re-read the ticket as it stands, triage_ticket when the discussion re-scores or re-scopes the work, comment for interim notes, and report_outcome when the work is done — the outcome report is what moves the ticket forward, and before sending one you self-review against the ticket\u{2019}s requirements.
Several people may write here. Answer whoever you are answering by name, keep replies as short as the thread\u{2019}s own register, and never speak for a person.";

const WORK_CAP: usize = 2_000;

/// The live half of the prompt: which ticket, what state it is in, what the
/// work is. Appended to `TICKET_MODE_PROMPT` (leading blank line included,
/// like `plan_routing_block`). Pure — takes the head, not the pool — so the
/// caller pays for one read and the block is testable without a database.
pub fn ticket_context_block(head: &TicketHead) -> String {
    let mut parts = vec![format!(
        "The ticket: {} (status: {}).",
        head.line(),
        head.status
    )];
    if let Some(work) = head.description.as_deref().filter(|d| !d.trim().is_empty()) {
        parts.push(format!("The work:\n{}", truncate_utf16(work, WORK_CAP)));
    }
    format!("\n\n{}", parts.join("\n\n"))
}

// ── The gate ─────────────────────────────────────────────────────────────────

/// The cases the model never sees, because the answer is structural.
/// Some(relevant) settles the message; None hands it to the judge.
///
/// An ATTACHMENTS-ONLY turn is a handoff — someone did what the agent asked
/// and posted the file — and there is no text to judge, so it is relevant
/// by construction. The degenerate empty-and-bare turn (no text, nothing
/// attached) is the one message the agent should never answer, and the one
/// shape that must not reach the fail-open judge, which would have to say
/// true.
///
/// A message that NAMES THE TICKET REF ("WEB-31 is still 500ing") is about
/// this ticket by construction — the ref is unique to the board, a message
/// carrying it was written about this work, and spending a judge call to
/// rediscover that is the gate taxing its own obvious case.
fn structural_relevance(head: &TicketHead, message: &str, attachments: usize) -> Option<bool> {
    let text = message.trim();
    if text.is_empty() {
        return Some(attachments > 0);
    }
    if head.ticket_ref.as_deref().is_some_and(|r| text.contains(r)) {
        return Some(true);
    }
    None
}

/// Whether the assigned agent should turn on this message. Cheap structural
/// cases first (above), then the `ticket-relevance` harness — the org's
/// cheap chore seat, never the ticket agent's own model — with every
/// failure folding to true: an unroutable judge, an unparseable verdict, a
/// boolean the schema rejected — none of these may be the reason a human
/// message goes unanswered.
///
/// `recent` is the discussion immediately before the message, one
/// "user: …" / "assistant: …" per entry, oldest first; the caller clips it
/// (the gate is one cheap call, not a transcript re-read).
pub async fn ticket_message_relevant(
    state: &AppState,
    head: &TicketHead,
    message: &str,
    attachments: usize,
    recent: &[String],
) -> bool {
    if let Some(relevant) = structural_relevance(head, message, attachments) {
        return relevant;
    }
    let input = json!(TicketRelevanceInput {
        ticket: head.line(),
        work: head.description.clone(),
        message: message.to_string(),
        recent: recent.to_vec(),
    });
    run_harness(
        state,
        &ticket_relevance_harness(),
        &input,
        RunContext {
            caller: "platform:ticket-relevance".into(),
            ..RunContext::default()
        },
    )
    .await
    // THE FAIL-OPEN FOLD. Every way this can come back empty — harness
    // error, null verdict, a value shaped like anything but
    // {"relevant": bool} — is true. The gate may cost an unneeded reply;
    // it may never cost an unanswered one.
    .ok()
    .and_then(|r| r.value)
    .and_then(|v| v.get("relevant").and_then(Value::as_bool))
    .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> TicketHead {
        TicketHead {
            ticket_ref: Some("WEB-31".into()),
            title: "Retry failed webhook deliveries".into(),
            status: "doing".into(),
            description: Some("Retries with backoff; give up after three.".into()),
        }
    }

    #[test]
    fn the_structural_cases_never_reach_the_model() {
        let h = head();
        // An attachments-only turn is a handoff — relevant by construction.
        assert_eq!(structural_relevance(&h, "  ", 1), Some(true));
        // Bare emptiness is the one message the agent must not answer, and
        // the one shape the fail-open judge would have gotten wrong.
        assert_eq!(structural_relevance(&h, "", 0), Some(false));
        // Naming the ref is about this ticket by construction — the
        // whitespace-only trim matters more than the case.
        assert_eq!(
            structural_relevance(&h, "WEB-31 is still 500ing on staging", 0),
            Some(true)
        );
        // Everything else is the judge's to decide.
        assert_eq!(
            structural_relevance(&h, "Does anyone know if the café is open?", 0),
            None
        );
    }

    #[test]
    fn a_board_without_numbering_gates_on_the_title_alone() {
        let unnumbered = TicketHead {
            ticket_ref: None,
            ..head()
        };
        // No ref to name, so nothing structural short-circuits on the ref —
        // the line still renders, and the judge still sees the room.
        assert_eq!(
            structural_relevance(&unnumbered, "WEB-31 is still 500ing on staging", 0),
            None
        );
        assert_eq!(
            unnumbered.line(),
            "Retry failed webhook deliveries".to_string()
        );
    }

    #[test]
    fn the_context_block_leads_with_the_line_and_carries_the_work() {
        let block = ticket_context_block(&head());
        assert!(
            block.starts_with(
                "\n\nThe ticket: WEB-31 — Retry failed webhook deliveries (status: doing)."
            ),
            "{block}"
        );
        assert!(block.contains("The work:\nRetries with backoff"));
        // An empty description is skipped, not rendered as an empty block.
        let bare = ticket_context_block(&TicketHead {
            description: Some("   ".into()),
            ..head()
        });
        assert!(!bare.contains("The work:"));
    }
}

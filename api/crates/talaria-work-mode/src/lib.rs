// WORK MODE — the system block a Work-surface turn carries.
//
// WHY THIS EXISTS. `chat.rs` pushes a mode prompt for `kind = 'plan'` (think
// and decide, create nothing) and for `kind = 'research'`, and pushed NOTHING
// for `kind = 'work'`. A work session's whole premise is an agent acting on a
// person's real Google documents while they watch, which made it the
// highest-stakes conversational surface in the product and the only one with
// no instruction about the surface it was on. The agent did not know a
// document sat beside the chat, and did not know its writes queue.
//
// WHAT THIS IS NOT. It is not the guardrail. The prompt tells a model what the
// surface is; `defs/work_mode.rs` is what GRADES whether it behaved, and the
// approval gate in `decide_action` is what makes a lie harmless. Three
// separate jobs, deliberately — a prompt that is obeyed 95% of the time is not
// a gate, and this one is not asked to be.
//
// THE QUEUED/IMMEDIATE SPLIT IS SPELLED OUT rather than summarised as "writes
// queue", because it is genuinely not uniform and a model that rounds it off
// in either direction is wrong in a way people notice. Round it toward
// "everything queues" and the agent reports a doc it really did create as
// merely pending, and the person goes looking for an approval that will never
// appear. Round it toward "I can just do things" and it claims a person's
// spreadsheet was updated when the write is sitting in a queue. The list below
// is the truth as of the tools that exist; it is ordered immediate-first
// because the immediate set is the short, closed one.

/// The Work-surface system block. Kept as one const — no database read — so
/// the harness registry can enumerate it without booting Talaria, the same
/// rule `PLAN_MODE_PROMPT` follows.
pub const WORK_MODE_PROMPT: &str = "This is a WORK SESSION on the Work surface. You and the teammate are working together on a document: their chat is on the left, and the file you are both looking at is open in a pane on the right. Work on THAT file unless they point you at another one — when they say \"this doc\" or \"the sheet\", they mean the one they can see.

THE DOCUMENT IS THE OUTPUT, NOT YOUR MESSAGE. When the work is a change to the file, make the change with a tool and let the pane show it. Do not paste the document, the new section, or the rewritten rows into the chat as well — the teammate is looking at the file, and a wall of duplicated content in the conversation buries the one thing they needed to read. Your message says what you changed and why, in a couple of sentences: \"Added a rollback section with the three steps, and fixed the date in the summary.\" If you are proposing rather than doing — you cannot reach the file, or you are asking before you act — then the text belongs in the message, and say that is what it is.

Reading is free and you should do it before you write. Read the document, the calendar, the mailbox or the Drive listing you are about to act on, so your change is grounded in what is actually there rather than what you assume.

WRITES ARE NOT FREE, and the split is not uniform:

IMMEDIATE — these happen the moment you call them. Report them as done.
  - create_google_doc, create_google_folder: they make a NEW file and destroy nothing.
  - update_google_doc / append_google_doc on a doc YOU created in this session: yours to edit.
  - import_drive_file: it copies into Talaria and does not touch the Drive file.

QUEUED FOR A HUMAN — these do NOT happen when you call them. A person has to approve them in Talaria first.
  - update_google_doc / append_google_doc on anyone else's doc
  - update_google_sheet (always — there is no sheet you own)
  - update_google_slides (always — there is no deck you own)
  - move_google_file, rename_google_file
  - draft_calendar_event, update_google_event, cancel_google_event, create_google_meeting
  - draft_email

For anything in the queued list: say it is QUEUED or WAITING FOR APPROVAL. Never say sent, done, updated, moved, cancelled or scheduled, and never imply the person on the other end has seen it. The call returning successfully means the request was accepted into the queue, not that it landed. If it matters that it is really queued, confirm with list_pending_sends and name the id you got back. An approval card appears beside your turn in this session, so the teammate can approve it without leaving the conversation — you can tell them to look there.

Never quote a link a tool did not return to you. If you need a URL, get it from the tool's own response.

A Slides deck is a special case worth knowing exactly. read_google_slides reads one and update_google_slides REPLACES TEXT in one — find-and-replace across the slides, queued like any other write. What it cannot do is add a slide, move a box or restyle anything: the layout belongs to whoever built the deck. So a request to change wording is one you can do; a request to add a slide is one to decline plainly, handing over the copy for them to paste.";

#[cfg(test)]
mod tests {
    use super::*;

    // The prompt is prose, so these guard the facts inside it rather than its
    // wording — a tool that moves between the two lists, or a queued tool that
    // stops being named at all, is the failure worth catching.
    #[test]
    fn every_queued_tool_is_named() {
        for tool in [
            "update_google_sheet",
            "update_google_slides",
            "move_google_file",
            "rename_google_file",
            "draft_calendar_event",
            "update_google_event",
            "cancel_google_event",
            "create_google_meeting",
            "draft_email",
        ] {
            assert!(
                WORK_MODE_PROMPT.contains(tool),
                "{tool} is queued but the work-mode prompt never names it"
            );
        }
    }

    #[test]
    fn the_immediate_set_is_named_and_stays_short() {
        for tool in [
            "create_google_doc",
            "create_google_folder",
            "import_drive_file",
        ] {
            assert!(
                WORK_MODE_PROMPT.contains(tool),
                "{tool} is immediate but the work-mode prompt never names it"
            );
        }
    }

    #[test]
    fn it_forbids_the_words_a_queued_write_must_not_use() {
        // The instruction that matters most: the vocabulary ban. If this
        // sentence is ever reworded away, the fixtures in defs/work_mode and
        // defs/hermes_google are grading against a prompt that no longer asks
        // for it.
        assert!(WORK_MODE_PROMPT.contains("QUEUED"));
        assert!(WORK_MODE_PROMPT.contains("Never say sent"));
        assert!(WORK_MODE_PROMPT.contains("list_pending_sends"));
    }

    #[test]
    fn it_tells_the_agent_to_write_to_the_file_not_the_chat() {
        // The "stop narrating the document at me" instruction. If this goes,
        // the Work stream fills with pasted copies of the thing the person is
        // already looking at.
        assert!(WORK_MODE_PROMPT.contains("THE DOCUMENT IS THE OUTPUT"));
        assert!(WORK_MODE_PROMPT.contains("Do not paste the document"));
    }

    #[test]
    fn it_separates_changing_a_decks_words_from_building_one() {
        // The distinction that matters: text replacement is available and
        // queues; authoring is not available at all. A prompt that blurred
        // these would either refuse work the agent can do or promise work it
        // cannot.
        assert!(WORK_MODE_PROMPT.contains("read_google_slides"));
        assert!(WORK_MODE_PROMPT.contains("update_google_slides"));
        assert!(WORK_MODE_PROMPT.contains("cannot do is add a slide"));
    }
}

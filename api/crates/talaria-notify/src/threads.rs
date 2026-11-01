// Conversations and channels as notification audiences: member ids, the
// fan events, where a thread's rows point, the read sweep, and the agent
// reply writer.

use sqlx::PgPool;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;
use crate::brief::mark_brief_stale;
use crate::inbox::mark_read_by_href;
use crate::write::{NotificationInput, add_notification};

/// The thread producers' own feature flags (I-23). W2-P adds its own here.
pub const FEATURES: &[&str] = &[];

/// The members of a channel — who a message in it might be about.
///
/// Kept here rather than in channels.rs to break the cycle: channels' insert
/// needs the brief nudge, and the brief's delegation layer needs channels'
/// insert. It is one query and it is the only thing this
/// module needs to know about a channel.
pub async fn channel_member_ids(
    pg: &sqlx::PgPool,
    channel_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("select user_id::text from channel_members where channel_id = $1::uuid")
            .bind(channel_id)
            .fetch_all(pg)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// A message landed in a conversation. Detached on purpose: the person who
/// sent it is waiting on the POST that wrote it, and their reply must not
/// block on somebody else's brief bookkeeping.
///
/// WHAT IT DOES NOT DO IS SWEEP. A sweep is four scoped queries and up to one
/// model call, and posting a message in a busy DM would fire one per message
/// per participant. This clears the sweep throttle and rings the bell instead:
/// an open page refetches, the read path sweeps on the way through because the
/// throttle is now clear, and the person sees the line close. A page nobody is
/// looking at costs one UPDATE and picks the change up on the next scheduled
/// pass.
pub fn briefs_follow_message(deps: NotifyDeps, channel_id: String) {
    tokio::spawn(async move {
        match channel_member_ids(&deps.pg, &channel_id).await {
            Ok(ids) => {
                if let Err(e) = mark_brief_stale(&deps, &ids).await {
                    tracing::error!(
                        "[daily-brief] could not follow a message in {channel_id}: {e}"
                    );
                }
            }
            Err(e) => {
                tracing::error!("[daily-brief] could not follow a message in {channel_id}: {e}");
            }
        }
    });
}

/// Who can read a conversation: its owner, plus a plan's members. Chats have
/// no members — the owner alone. Ticket threads are channels now — their
/// audience is board membership, fanned through the channel shapes below.
/// Kept beside `channel_member_ids` for the same cycle-breaking reason: the
/// writer that fans needs it, and it is one query.
pub async fn conversation_audience_ids(
    pg: &sqlx::PgPool,
    conversation_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select user_id::text from conversations where id = $1::uuid \
         union \
         select user_id::text from conversation_members where conversation_id = $1::uuid",
    )
    .bind(conversation_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Ring every member's firehose: a message landed in this channel. Id-shaped
/// and detached, like the brief nudge it stands beside — a message in a room
/// of N members is N publishes of a few dozen bytes each, and none of them
/// may cost the post that caused them. The event reaches the AUTHOR too:
/// their page refetches a list whose unread count already excludes their own
/// turn, which is a no-op by design.
pub fn fan_channel_event(deps: NotifyDeps, channel_id: String) {
    tokio::spawn(async move {
        match channel_member_ids(&deps.pg, &channel_id).await {
            Ok(ids) => {
                for id in ids {
                    publish_user(
                        &deps.realtime,
                        &id,
                        &UserEvent::Channel {
                            channel_id: channel_id.clone(),
                        },
                    );
                }
            }
            Err(e) => {
                tracing::error!("[realtime] could not fan a channel event for {channel_id}: {e}");
            }
        }
    });
}

/// Ring owner and members alike: a turn landed in this conversation. The
/// unread pill it moves belongs to whoever was NOT looking — the author's own
/// client refetches and finds nothing new, the plan collaborator's finds their
/// teammate's turn, the thread's owner finds the agent's reply.
pub fn fan_conversation_event(deps: NotifyDeps, conversation_id: String) {
    tokio::spawn(async move {
        match conversation_audience_ids(&deps.pg, &conversation_id).await {
            Ok(ids) => {
                for id in ids {
                    publish_user(
                        &deps.realtime,
                        &id,
                        &UserEvent::Conversation {
                            conversation_id: conversation_id.clone(),
                        },
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    "[realtime] could not fan a conversation event for {conversation_id}: {e}"
                );
            }
        }
    });
}

/// Fan a conversation event to an audience already resolved. Hard delete uses
/// this: the usual fan reads the row, and that row is about to be gone.
pub fn fan_known_conversation(deps: NotifyDeps, conversation_id: String, user_ids: Vec<String>) {
    tokio::spawn(async move {
        for id in user_ids {
            publish_user(
                &deps.realtime,
                &id,
                &UserEvent::Conversation {
                    conversation_id: conversation_id.clone(),
                },
            );
        }
    });
}

/// Where a conversation's notifications point: the thread itself for a chat,
/// the plan for a plan, and for a research discussion the RUN — opening the
/// run is research's seen gesture (it marks this href read), so the bell row
/// and the rail badge clear in the same click. A research conversation whose
/// run is gone has nowhere to point (`None`) and files nothing. Shared by the
/// writer that files these rows and the reader that clears them, so the two
/// ends of the href can never drift apart.
pub async fn conversation_notification_href(
    pg: &PgPool,
    kind: &str,
    agent_model: &str,
    conversation_id: &str,
) -> Option<String> {
    if kind == "research" {
        let run_id: Option<String> =
            sqlx::query_as("select id::text from research_runs where conversation_id = $1::uuid")
                .bind(conversation_id)
                .fetch_optional(pg)
                .await
                .ok()
                .flatten()
                .map(|(id,): (String,)| id);
        return notification_href(kind, agent_model, conversation_id, run_id.as_deref(), None);
    }
    if kind == "ticket" {
        let task: Option<(String, String)> = sqlx::query_as(
            "select board_id::text, id::text from tasks where conversation_id = $1::uuid",
        )
        .bind(conversation_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
        return notification_href(
            kind,
            agent_model,
            conversation_id,
            None,
            task.as_ref().map(|(b, t)| (b.as_str(), t.as_str())),
        );
    }
    notification_href(kind, agent_model, conversation_id, None, None)
}

/// The pure half of the href — everything but the lookups (research's run,
/// the ticket's board and task), so the formats the bell's rows and the UI's
/// routes must agree on are pinned by a DB-free test.
fn notification_href(
    kind: &str,
    agent_model: &str,
    conversation_id: &str,
    run_id: Option<&str>,
    ticket: Option<(&str, &str)>,
) -> Option<String> {
    match kind {
        "chat" => Some(format!("/comms/agent/{agent_model}/{conversation_id}")),
        "plan" => Some(format!("/plan/{conversation_id}")),
        "research" => run_id.map(|r| format!("/research/{r}")),
        // The task's own permalink — where the thread lives as a tab.
        "ticket" => ticket.map(|(b, t)| format!("/boards/{b}/{t}")),
        _ => None,
    }
}

/// Reading a thread is the end of its bell rows. The read cursor's route
/// calls this once the advanced cursor covers the thread's latest turn:
/// every unread notification pointing at the thread (its href, whatever
/// kind filed it) is now seen — the row's whole job was to draw the reader
/// here. Returns the ids it cleared, so the caller can publish the read-sync
/// event (and tell the client whether the bell owes it a refetch). A reply landing DURING the sweep
/// files its own fresh row after — the writer's cursor gate reads the
/// already-advanced cursor, and a reply newer than that cursor rightly
/// rings again.
pub async fn clear_thread_notification_ids(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("select kind, agent_model from conversations where id = $1::uuid")
            .bind(conversation_id)
            .fetch_optional(pg)
            .await?;
    let Some((kind, agent_model)) = row else {
        return Ok(Vec::new()); // the access gate already proved the thread exists
    };
    let Some(href) = conversation_notification_href(pg, &kind, &agent_model, conversation_id).await
    else {
        return Ok(Vec::new());
    };
    mark_read_by_href(pg, user_id, &href).await
}

/// `clear_thread_notification_ids`, answering how many — wave 1's signature,
/// kept for its callers.
pub async fn clear_thread_notifications(
    pg: &PgPool,
    user_id: &str,
    conversation_id: &str,
) -> Result<u64, sqlx::Error> {
    clear_thread_notification_ids(pg, user_id, conversation_id)
        .await
        .map(|ids| ids.len() as u64)
}

/// An agent reply that landed while its readers were away: file ONE
/// agent-reply notification per audience member whose read cursor does not
/// cover the reply's seq. The cursor is the "still looking" gate — someone
/// watching the thread has auto-advanced past the reply and gets nothing —
/// and the dedupe is the pile-up gate: while one row per thread sits unread,
/// further replies fold into it (the DM shape, `notify_dm_message`). The
/// bell, toasts, mail, and the brief all ride this single writer, so this is
/// the whole fan-out: no surface-specific code anywhere.
///
/// Reads the reply AS SAVED — the caller runs this after the confab guard,
/// so strict mode's scrub has already reached the `messages` row and the
/// notification body can only ever carry what the transcript itself carries.
/// An empty or not-yet-complete reply notifies nobody. Detached by the
/// caller, like every fan on this path.
pub async fn notify_agent_reply(deps: &NotifyDeps, conversation_id: &str, message_id: &str) {
    // Kind decides where the row points; the model names who replied. One
    // joined read, and only for a reply that actually landed complete.
    let head: Option<(String, String, String, i32)> = sqlx::query_as(
        "select c.kind, c.agent_model, m.content, m.seq \
         from conversations c join messages m on m.conversation_id = c.id \
         where c.id = $1::uuid and m.id = $2::uuid and m.role = 'assistant' \
           and m.status = 'complete'",
    )
    .bind(conversation_id)
    .bind(message_id)
    .fetch_optional(&deps.pg)
    .await
    .ok()
    .flatten();
    let Some((kind, agent_model, content, reply_seq)) = head else {
        return;
    };
    if content.trim().is_empty() {
        return;
    }
    let Some(href) =
        conversation_notification_href(&deps.pg, &kind, &agent_model, conversation_id).await
    else {
        return; // nothing for a row to point at — research whose run is gone
    };
    // The audience WITH cursors, the -1 floor shared with the unread counts:
    // a member with no cursor row has read nothing, and seq 0 is a real turn.
    let audience: Vec<(String, i32)> = sqlx::query_as(
        "select a.id::text, coalesce(cr.last_read_seq, -1) from ( \
            select user_id as id from conversations where id = $1::uuid \
            union \
            select user_id from conversation_members where conversation_id = $1::uuid \
         ) a \
         left join conversation_reads cr on cr.conversation_id = $1::uuid and cr.user_id = a.id",
    )
    .bind(conversation_id)
    .fetch_all(&deps.pg)
    .await
    .unwrap_or_default();
    let label = talaria_fleet_layout::describe_agent(&agent_model).label;
    // 200 UTF-16 units, '…' only past the bound — the DM body contract.
    let body = if talaria_body::utf16_len(&content) > 200 {
        format!("{}…", talaria_body::truncate_utf16(&content, 200))
    } else {
        content.clone()
    };
    let title = format!("{label} replied");
    for (member, cursor) in audience {
        if reply_seq <= cursor {
            continue; // they have read past this reply — still looking
        }
        let pending: Option<i32> = sqlx::query_scalar(
            "select 1 from notifications \
             where user_id = $1::uuid and kind = 'agent-reply' and href = $2 \
               and read_at is null limit 1",
        )
        .bind(&member)
        .bind(&href)
        .fetch_optional(&deps.pg)
        .await
        .ok()
        .flatten();
        if pending.is_some() {
            continue; // one unread pointer per thread; this reply folds in
        }
        let _ = add_notification(
            deps,
            &member,
            &NotificationInput {
                kind: "agent-reply",
                title: &title,
                body: Some(&body),
                href: Some(&href),
            },
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_notification_hrefs_are_pinned() {
        // The writer files these exact strings and the read sweep matches
        // them exactly — the UI's routes must keep resolving all four.
        assert_eq!(
            notification_href("chat", "margaret-marketing", "c-1", None, None).as_deref(),
            Some("/comms/agent/margaret-marketing/c-1")
        );
        assert_eq!(
            notification_href("plan", "claude-x", "c-2", None, None).as_deref(),
            Some("/plan/c-2")
        );
        assert_eq!(
            notification_href("research", "claude-x", "c-3", Some("run-9"), None).as_deref(),
            Some("/research/run-9")
        );
        assert_eq!(
            notification_href("ticket", "claude-x", "c-5", None, Some(("b-1", "t-2"))).as_deref(),
            Some("/boards/b-1/t-2")
        );
        // A research discussion with no run behind it has nothing to point
        // at; a ticket whose task is gone (the orphan a delete leaves) has
        // nowhere to land either; an unknown kind is nobody's thread.
        assert_eq!(
            notification_href("research", "claude-x", "c-3", None, None),
            None
        );
        assert_eq!(
            notification_href("ticket", "claude-x", "c-5", None, None),
            None
        );
        assert_eq!(
            notification_href("other", "claude-x", "c-4", None, None),
            None
        );
    }
}

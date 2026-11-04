// Conversations and channels as notification audiences: member ids and
// their notify levels, the fan events, where a thread's rows point, the read
// sweep, and the agent reply writer. The producers' story — group keys,
// levels, participants — is docs/notifications/producers.md.

use sqlx::PgPool;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;
use crate::brief::mark_brief_stale;
use crate::inbox::mark_read_by_href;
use crate::meta::{ActorKind, Coalesce, NotifyActor, NotifyEntity, NotifyMeta, entity, group_key};
use crate::write::{NotificationInput, add_notification_with};

/// The producers' feature flags (I-23): rows coalesce by group key with a
/// count (`coalesce`), channel levels and timed mutes are obeyed
/// (`channel-levels`; `channel-levels-store` is the contract's storage), and
/// conversations you take part in notify you (`participants`: ticket
/// comments, thread replies).
pub const FEATURES: &[&str] = &["coalesce", "channel-levels", "participants"];

// ── Channel levels (O-14) ────────────────────────────────────────────────────

/// How much of one channel a person hears (`channel_members.notify_level`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelLevel {
    /// Every message: a DM row per message, a `channel-message` row in a
    /// channel (one per channel, counting).
    All,
    /// @mentions, and threads they take part in.
    Mentions,
    /// Nothing at all, @mentions included.
    Nothing,
}

impl ChannelLevel {
    /// The stored and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelLevel::All => "all",
            ChannelLevel::Mentions => "mentions",
            ChannelLevel::Nothing => "none",
        }
    }

    /// The stored level, else the channel kind's default: a DM notifies on
    /// every message, a channel on @mentions (O-14). An unknown stored value
    /// (the route never writes one) reads as the default, never as silence.
    pub fn of(stored: Option<&str>, channel_kind: &str) -> Self {
        match stored {
            Some("all") => ChannelLevel::All,
            Some("mentions") => ChannelLevel::Mentions,
            Some("none") => ChannelLevel::Nothing,
            _ if channel_kind == "dm" => ChannelLevel::All,
            _ => ChannelLevel::Mentions,
        }
    }
}

/// One member of a channel as its producers see them: their level, and
/// whether a timed mute is running (a muted row lands quietly — counted,
/// never toasted, bannered, pushed or mailed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelListener {
    pub user_id: String,
    pub level: ChannelLevel,
    pub muted: bool,
}

/// A channel's kind and every member row's level and mute, in ONE read — the
/// producers of a post ask this once, whatever the member count. A person who
/// reaches the channel through a team (`channel_teams`) has no member row, so
/// no level of their own, and is not listed: that gap is an owner decision
/// (docs/notifications/producers.md, S10). `None` kind: no such channel.
pub async fn channel_listeners(
    pg: &PgPool,
    channel_id: &str,
) -> Result<(Option<String>, Vec<ChannelListener>), sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "select c.kind, cm.user_id::text, cm.notify_level, \
                coalesce(cm.muted_until > now(), false) \
         from channels c left join channel_members cm on cm.channel_id = c.id \
         where c.id = $1::uuid",
    )
    .bind(channel_id)
    .fetch_all(pg)
    .await?;
    let kind = rows.first().map(|r| r.0.clone());
    let listeners = rows
        .into_iter()
        .filter_map(|(kind, user, level, muted)| {
            user.map(|user_id| ChannelListener {
                user_id,
                level: ChannelLevel::of(level.as_deref(), &kind),
                muted,
            })
        })
        .collect();
    Ok((kind, listeners))
}

// ── Who a message addresses (@mentions) ─────────────────────────────────────
// The token rules every mention producer asks (talaria-mentions files the
// rows); here because the ticket's comment participants must leave out the
// people a comment @mentions, and that producer cannot depend on
// talaria-mentions.

/// One mention token in the grammar the content scanner reads: lowercase,
/// every char outside [a-z0-9] mapped to '-', dash runs collapsed, edges
/// trimmed. The composer's insert (ui mentions.svelte.ts userMentionInsert)
/// speaks this same rule, so an '@' written from either side resolves.
fn normalize_token(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else {
            if out.ends_with('-') {
                continue;
            }
            out.push('-');
        }
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Tokens a person answers to: email localpart (in the scanner's grammar,
/// see `normalize_token`), dashed full name, first name.

pub fn user_mention_tokens(name: Option<&str>, email: Option<&str>) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    // The localpart must speak the SAME grammar as the content scanner
    // (mention_tokens): lowercase, every char outside [a-z0-9] mapped to '-',
    // dash runs collapsed, edges trimmed. A raw dotted localpart like
    // "j.siegel" could never be matched by the scanner's tokens — '@j.siegel'
    // scans to 'j' — so the person would silently never resolve (TALA-93).
    if let Some(email) = email
        && let Some(local) = email.split('@').next()
    {
        let local = normalize_token(local.to_lowercase());
        if !local.is_empty() && !tokens.contains(&local) {
            tokens.push(local);
        }
    }
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        let n = n.to_lowercase();
        let dashed = n.split_whitespace().collect::<Vec<_>>().join("-");
        if !dashed.is_empty() && !tokens.contains(&dashed) {
            tokens.push(dashed);
        }
        if let Some(first) = n.split_whitespace().next()
            && !tokens.contains(&first.to_string())
        {
            tokens.push(first.to_string());
        }
    }
    tokens
}

/// The @tokens present in a body (lowercased, without the leading @): the
/// first char after @ must be alphanumeric, then alphanumerics and dashes.
pub fn mention_tokens(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && i + 1 < bytes.len() {
            let rest = &content[i + 1..];
            let mut chars = rest.char_indices();
            let valid_first = chars.next().is_some_and(|(_, c)| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c.is_ascii_uppercase()
            });
            if valid_first {
                let mut end = rest.len();
                for (idx, c) in rest.char_indices() {
                    let ok = c.is_ascii_lowercase()
                        || c.is_ascii_uppercase()
                        || c.is_ascii_digit()
                        || c == '-';
                    if !ok {
                        end = idx;
                        break;
                    }
                }
                let token = rest[..end].to_lowercase();
                if !token.is_empty() && !out.contains(&token) {
                    out.push(token);
                }
                // Resume scanning after the token — a '-' run inside one
                // token cannot start another.
                i += 1 + end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Does a person with this name and email answer to any of these @tokens?
pub fn answers_to_tokens(name: Option<&str>, email: Option<&str>, tokens: &[String]) -> bool {
    !tokens.is_empty()
        && user_mention_tokens(name, email)
            .iter()
            .any(|t| tokens.contains(t))
}

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

/// An agent reply that landed while its readers were away: ONE agent-reply
/// row per audience member per thread (`group_key::thread`), for every member
/// whose read cursor does not cover the reply's seq. The cursor is the "still
/// looking" gate — someone watching the thread has auto-advanced past the
/// reply and gets nothing. A reply landing while that person's row sits
/// unread COALESCES into it (the DM shape): the newest reply's body, `count +
/// 1`, and a re-alert. Wave 1 skipped it instead, and a second reply never
/// rang. A row filed before the contract (no key) is adopted into the group,
/// so it keeps its id. A read row has left the group, so the next reply files
/// a fresh one. The bell, toasts, mail, and the brief all ride this single
/// writer, so this is the whole fan-out: no surface-specific code anywhere.
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
    let input = NotificationInput {
        kind: "agent-reply",
        title: &title,
        body: Some(&body),
        href: Some(&href),
    };
    let meta = agent_reply_meta(conversation_id, &agent_model, &label);
    for (member, cursor) in audience {
        if reply_seq <= cursor {
            continue; // they have read past this reply — still looking
        }
        if let Err(e) = add_notification_with(deps, &member, &input, &meta).await {
            tracing::error!("[notifications] agent-reply row for {member} failed: {e}");
        }
    }
}

/// How an agent reply files: one open row per thread, re-alerting, adopting a
/// pre-contract row, about the conversation, from the agent. Pure, so the
/// shape is pinned by a test.
fn agent_reply_meta(conversation_id: &str, agent_model: &str, label: &str) -> NotifyMeta {
    NotifyMeta {
        group_key: Some(group_key::thread(conversation_id)),
        coalesce: Coalesce::Realert,
        adopt_legacy: true,
        entity: Some(NotifyEntity::new(entity::CONVERSATION, conversation_id)),
        actor: Some(NotifyActor {
            kind: ActorKind::Agent,
            id: agent_model.to_string(),
            label: label.to_string(),
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_default_by_kind_and_read_what_is_stored() {
        // O-14: a DM hears everything, a channel its @mentions.
        assert_eq!(ChannelLevel::of(None, "dm"), ChannelLevel::All);
        assert_eq!(ChannelLevel::of(None, "channel"), ChannelLevel::Mentions);
        assert_eq!(ChannelLevel::of(None, "group"), ChannelLevel::Mentions);
        // A stored level wins over the kind, both ways.
        assert_eq!(ChannelLevel::of(Some("none"), "dm"), ChannelLevel::Nothing);
        assert_eq!(ChannelLevel::of(Some("all"), "channel"), ChannelLevel::All);
        assert_eq!(
            ChannelLevel::of(Some("mentions"), "dm"),
            ChannelLevel::Mentions
        );
        // Garbage is the default, never silence.
        assert_eq!(ChannelLevel::of(Some("loud"), "dm"), ChannelLevel::All);
        for l in [
            ChannelLevel::All,
            ChannelLevel::Mentions,
            ChannelLevel::Nothing,
        ] {
            assert_eq!(ChannelLevel::of(Some(l.as_str()), "channel"), l);
        }
        assert_eq!(ChannelLevel::Nothing.as_str(), "none");
    }

    #[test]
    fn an_agent_reply_files_into_its_thread_group() {
        let m = agent_reply_meta("c-1", "margaret-marketing", "Margaret");
        assert_eq!(m.group_key.as_deref(), Some("thread:c-1"));
        assert_eq!(m.coalesce, Coalesce::Realert);
        assert!(m.adopt_legacy && !m.quiet && m.group_window_secs.is_none());
        assert_eq!(m.entity, Some(NotifyEntity::new("conversation", "c-1")));
        let a = m.actor.unwrap();
        assert_eq!(
            (a.kind, a.id.as_str(), a.label.as_str()),
            (ActorKind::Agent, "margaret-marketing", "Margaret")
        );
        assert_eq!(FEATURES, ["coalesce", "channel-levels", "participants"]);
    }

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

    // TALA-93: the UI inserts the email localpart as the composer token, and
    // the content scanner's grammar is [a-z0-9-] — so both sides must speak
    // the same normalized token. These tests pin the agreement.

    #[test]
    fn user_mention_tokens_normalize_the_localpart_to_the_grammar() {
        // What a person answers to is the localpart with every char outside
        // [a-z0-9] mapped to '-', runs collapsed, edges trimmed — never the
        // raw localpart, which '@j.siegel' could not match.
        assert_eq!(
            user_mention_tokens(Some("Jon Siegel"), Some("j.siegel@x.com")),
            ["j-siegel", "jon-siegel", "jon"]
        );
        assert_eq!(
            user_mention_tokens(Some("Sam Underscore"), Some("sam_siegel@x.com")),
            ["sam-siegel", "sam-underscore", "sam"]
        );
        assert_eq!(
            user_mention_tokens(Some("Pat Plus"), Some("p.siegel+ops@x.com")),
            ["p-siegel-ops", "pat-plus", "pat"]
        );
        // A localpart already in the grammar passes through unchanged.
        assert_eq!(
            user_mention_tokens(Some("Ada Lovelace"), Some("ada@x.com")),
            ["ada", "ada-lovelace"]
        );
        // No email: the name tokens alone, already conforming.
        assert_eq!(
            user_mention_tokens(Some("Grace Hopper"), None),
            ["grace-hopper", "grace"]
        );
    }

    #[test]
    fn the_token_the_picker_inserts_resolves_end_to_end() {
        // The agreement: insert (normalized) → '@' + insert → the content
        // scanner → answers_to_tokens must hold for every email shape.
        let cases: &[(&str, &str, &str)] = &[
            ("Jon Siegel", "j.siegel@x.com", "j-siegel"),
            ("Sam Underscore", "sam_siegel@x.com", "sam-siegel"),
            ("Pat Plus", "p.siegel+ops@x.com", "p-siegel-ops"),
            ("Ada Lovelace", "ada@x.com", "ada"),
        ];
        for (name, email, insert) in cases {
            let message = format!("@{insert}");
            let tokens = mention_tokens(&message);
            assert!(!tokens.is_empty(), "{message} must scan to a token");
            assert!(
                answers_to_tokens(Some(name), Some(email), &tokens),
                "{name} <{email}> must answer to {message} (scanned {tokens:?})"
            );
        }
    }
}

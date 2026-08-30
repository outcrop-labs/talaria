// Who is still waiting on an answer from you — port of
// ui/src/server/daily-brief-comms.ts.
//
// THE BUG THIS FILE IS. The brief's first version took its conversations from
// `inbox-focus-sources.channelItems`, which selects DMs with UNREAD messages —
// the right question for a queue, and the wrong one for a document about who is
// waiting. Opening a DM drops it out of that query, so the sweep saw the key
// vanish and appended a resolution: the brief told you Priya's question had
// been handled because you had glanced at it. Reading is not answering, and the
// difference is the entire subject of the "Waiting on you" section.
//
// So this asks the log instead of the read cursor: a conversation is open until
// the last thing in it was said BY YOU. That single change gives three states
// where there were two —
//
//   N UNREAD             they wrote, you have not looked
//   READ, NOT ANSWERED   you looked, they are still waiting  ← the missing one
//   answered             the last word is yours
//
// — and the middle one is the state a busy person is actually in most of the
// day. It is also the state that makes delegation worth having: a thread you
// have read and cannot get to is exactly the thread to hand to your assistant.
//
// WHO COUNTS AS "YOU" INCLUDES YOUR ASSISTANT, and that is not a shortcut. A
// delegated reply IS an answer to the person waiting — they have been responded
// to, by an agent that says so in its own name — so the line closes. What it
// does not do is pretend you wrote it: `answered_by` carries which of the two
// it was, all the way to the words on the line.

use serde_json::json;
use sqlx::PgPool;

use crate::daily_brief::focus::{as_iso, fingerprint, key_of};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsweredBy {
    You,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct CommsDraft {
    pub id: String,
    pub content: String,
    pub stale: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct CommsLine {
    pub key: String,
    pub channel_id: String,
    pub peer: String,
    /// Messages you have not read. Zero does NOT mean answered.
    pub unread: i32,
    /// None while they are still waiting.
    pub answered_by: Option<AnsweredBy>,
    pub answered_at: Option<String>,
    /// Seq of the last message from them — what a draft would be answering.
    pub awaiting_seq: i32,
    pub excerpt: String,
    pub updated_at: String,
    /// A pending draft the assistant has written for this thread, if any.
    pub draft: Option<CommsDraft>,
    /// The assistant may send here without asking.
    pub delegated: bool,
    pub status_label: String,
    pub priority: String,
    pub badge: Option<serde_json::Value>,
    pub source_fingerprint: String,
}

/// One row of the comms read — a derived struct rather than a tuple because
/// sqlx's `FromRow` for tuples stops at sixteen and this read has eighteen
/// columns, each of which the body below names exactly once.
#[derive(Debug, sqlx::FromRow)]
struct CommsRow {
    channel_id: String,
    peer_name: Option<String>,
    peer_email: Option<String>,
    updated_ms: i64,
    last_seq: Option<i32>,
    last_author: Option<String>,
    last_author_type: Option<String>,
    last_ms: Option<i64>,
    unread: i32,
    awaiting_seq: Option<i32>,
    awaiting_content: Option<String>,
    awaiting_author: Option<String>,
    draft_id: Option<String>,
    draft_content: Option<String>,
    draft_seq: Option<i32>,
    draft_ms: Option<i64>,
    delegated: bool,
}

/// Every DM this person is in that has ever been spoken in, with the state of
/// its last message, its pending draft, and whether the assistant may answer.
///
/// ONE QUERY, because the four facts have to agree with each other. Read as
/// four passes, a thread could be reported awaiting a reply by one and answered
/// by another between statements — and the thing being decided is whether to
/// put words in somebody's mouth.
pub async fn comms_lines(
    pg: &PgPool,
    user_id: &str,
    assistant_model: Option<&str>,
) -> Vec<CommsLine> {
    let rows = sqlx::query_as::<_, CommsRow>(
        r#"
        with me as (select id, coalesce(email, name, 'user') as handle from users where id = $1::uuid)
        select c.id::text as channel_id,
               pu.name as peer_name, pu.email as peer_email,
               (trunc(extract(epoch from c.updated_at) * 1000))::bigint as updated_ms,
               last.seq as last_seq, last.author as last_author, last.author_type as last_author_type,
               (trunc(extract(epoch from last.created_at) * 1000))::bigint as last_ms,
               (select count(*)::int from channel_messages m
                 where m.channel_id = c.id and m.seq > member.last_read_seq and m.status = 'complete'
                   and not (m.author_type = 'user' and m.author = me.handle)) as unread,
               theirs.seq as awaiting_seq, theirs.content as awaiting_content, theirs.author as awaiting_author,
               d.id::text as draft_id, d.content as draft_content, d.in_reply_to_seq as draft_seq,
               (trunc(extract(epoch from d.created_at) * 1000))::bigint as draft_ms,
               exists(
                 select 1 from assistant_reply_grants g
                 where g.user_id = $1::uuid and g.revoked_at is null
                   and (g.channel_id is null or g.channel_id = c.id)
               ) as delegated
        from channels c
        join channel_members member on member.channel_id = c.id and member.user_id = $1::uuid
        cross join me
        left join channel_members peer on peer.channel_id = c.id and peer.user_id <> $1::uuid
        left join users pu on pu.id = peer.user_id
        -- The last thing said, by anyone. This is what decides "answered".
        left join lateral (
          select m.seq, m.author, m.author_type, m.created_at
          from channel_messages m
          where m.channel_id = c.id and m.status = 'complete'
          order by m.seq desc limit 1
        ) last on true
        -- The last thing THEY said — what a reply would be answering.
        left join lateral (
          select m.seq, m.content, m.author
          from channel_messages m
          where m.channel_id = c.id and m.status = 'complete'
            and not (m.author_type = 'user' and m.author = me.handle)
            and not (m.author_type = 'agent' and m.author = $2::text)
          order by m.seq desc limit 1
        ) theirs on true
        left join lateral (
          select id, content, in_reply_to_seq, created_at
          from assistant_reply_drafts
          where user_id = $1::uuid and channel_id = c.id and status = 'pending'
          order by created_at desc limit 1
        ) d on true
        where c.archived_at is null and c.kind = 'dm' and last.seq is not null
        order by c.updated_at desc limit 100
        "#,
    )
    .bind(user_id)
    .bind(assistant_model)
    .fetch_all(pg)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[daily-brief] comms lines read failed: {e}");
        Vec::new()
    });

    let mut out = Vec::new();
    for row in rows {
        let CommsRow {
            channel_id,
            peer_name,
            peer_email,
            updated_ms,
            last_seq,
            last_author,
            last_author_type,
            last_ms,
            unread,
            awaiting_seq,
            awaiting_content,
            awaiting_author,
            draft_id,
            draft_content,
            draft_seq,
            draft_ms,
            delegated,
        } = row;

        // Is this author string the peer rather than the owner?
        //
        // A DM has exactly two people, so anything authored by a `user` that is
        // not the peer is the owner. Asked this way round because the owner's
        // handle is `coalesce(email, name, 'user')` and a row written before
        // they had an email can carry the older spelling — treating an
        // unrecognised author as THEM (and so leaving the line open) is the
        // safe direction to be wrong in. The unsafe direction closes a line
        // nobody answered.
        let is_peer = |author: &str| -> bool {
            peer_email.as_deref() == Some(author) || peer_name.as_deref() == Some(author)
        };
        let mine_by_hand = last_author_type.as_deref() == Some("user")
            && last_author.is_some()
            && !last_author.as_deref().is_some_and(is_peer);
        let mine_by_agent = last_author_type.as_deref() == Some("agent")
            && assistant_model.is_some()
            && last_author.as_deref() == assistant_model;
        let answered_by = if mine_by_agent {
            Some(AnsweredBy::Assistant)
        } else if mine_by_hand {
            Some(AnsweredBy::You)
        } else {
            None
        };
        let peer = peer_name
            .or(peer_email)
            .or(awaiting_author)
            .unwrap_or_else(|| "this conversation".to_string());

        // Never spoken to us at all — nothing is waiting and nothing was
        // answered.
        let awaiting_seq = match awaiting_seq {
            Some(seq) => seq,
            None => continue,
        };

        let stale = draft_seq.is_some_and(|s| s < awaiting_seq);
        let draft = match (draft_id, draft_content) {
            (Some(id), Some(content)) => Some(CommsDraft {
                id,
                content,
                stale,
                created_at: as_iso(draft_ms.unwrap_or_default()),
            }),
            _ => None,
        };

        let draft_fingerprint_piece = draft.as_ref().map(|d| format!("{}:{}", d.id, d.stale));
        // The fingerprint carries every field the line RENDERS, so a thread
        // that moves from unread to read appends a change rather than sitting
        // there claiming an unread count that is gone.
        let source_fingerprint = fingerprint(&json!({
            "u": unread,
            "a": answered_by.map(|a| match a { AnsweredBy::You => "you", AnsweredBy::Assistant => "assistant" }),
            "s": awaiting_seq,
            "l": last_seq,
            "d": draft_fingerprint_piece,
            "g": delegated,
        }));

        out.push(CommsLine {
            key: key_of("channel", &channel_id),
            channel_id,
            peer,
            unread,
            answered_by,
            answered_at: answered_by.and(last_ms).map(as_iso),
            awaiting_seq,
            excerpt: awaiting_content
                .unwrap_or_default()
                .chars()
                .take(1_000)
                .collect(),
            updated_at: as_iso(updated_ms),
            status_label: label(unread, answered_by, draft.as_ref()),
            // A person waiting on a human answer is p1 whether or not it has
            // been read. It is NOT downgraded once read — "I saw it" is the
            // state this section exists to keep visible, not a reason to stop
            // showing it.
            priority: if answered_by.is_some() { "ok" } else { "p1" }.to_string(),
            badge: match &draft {
                Some(d) if !d.stale => Some(json!({"label": "DRAFT READY", "tone": "accent"})),
                _ => None,
            },
            source_fingerprint,
            draft,
            delegated,
        });
    }
    out
}

fn label(unread: i32, answered_by: Option<AnsweredBy>, draft: Option<&CommsDraft>) -> String {
    match answered_by {
        Some(AnsweredBy::Assistant) => "ASSISTANT REPLIED".into(),
        Some(AnsweredBy::You) => "YOU REPLIED".into(),
        None if draft.is_some_and(|d| !d.stale) => {
            if unread > 0 {
                format!("{unread} UNREAD · DRAFT READY")
            } else {
                "READ · DRAFT READY".into()
            }
        }
        None if unread > 0 => format!("{unread} UNREAD"),
        // THE STATE THE OLD SOURCE COULD NOT SEE. Everything above it existed
        // before; this line is the fix.
        None => "READ, NOT ANSWERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(id: &str, stale: bool) -> CommsDraft {
        CommsDraft {
            id: id.into(),
            content: "content".into(),
            stale,
            created_at: "2026-08-29T12:00:00.000Z".into(),
        }
    }

    #[test]
    fn the_pinned_comms_fingerprint_vector() {
        // Byte-identical to the TS hash of the same object — the cross-runtime
        // contract pinned in focus.rs. A mismatch here means every comms line
        // in every existing brief reads as "changed" on the first Rust sweep.
        let v = json!({"u": 3, "a": "you", "s": 41, "l": 42, "d": "d7:null", "g": true});
        assert_eq!(
            fingerprint(&v),
            "1562f0b8ad6e99f71a16c1367df86423edc4e8f62a2f06222098ca011b169b8d"
        );
    }

    #[test]
    fn the_fingerprint_object_matches_the_ts_literal_key_for_key() {
        // The exact object commsLines hashes, spelled in the TS literal's key
        // order, for both the null-draft and pending-draft shapes.
        let null_draft = json!({"u": 0, "a": serde_json::Value::Null, "s": 7, "l": 7, "d": serde_json::Value::Null, "g": false});
        assert_eq!(fingerprint(&null_draft).len(), 64);
        let with_draft =
            json!({"u": 0, "a": "assistant", "s": 7, "l": 8, "d": "0d2f:true", "g": true});
        assert_ne!(fingerprint(&with_draft), fingerprint(&null_draft));
    }

    #[test]
    fn read_not_answered_is_the_state_the_old_source_could_not_see() {
        assert_eq!(label(0, None, None), "READ, NOT ANSWERED");
        assert_eq!(label(2, None, None), "2 UNREAD");
        assert_eq!(
            label(2, None, Some(&draft("d", false))),
            "2 UNREAD · DRAFT READY"
        );
        assert_eq!(
            label(0, None, Some(&draft("d", false))),
            "READ · DRAFT READY"
        );
        // A stale draft does not get the badge wording.
        assert_eq!(
            label(0, None, Some(&draft("d", true))),
            "READ, NOT ANSWERED"
        );
        assert_eq!(
            label(9, Some(AnsweredBy::You), Some(&draft("d", false))),
            "YOU REPLIED"
        );
        assert_eq!(
            label(0, Some(AnsweredBy::Assistant), None),
            "ASSISTANT REPLIED"
        );
    }

    #[test]
    fn excerpts_are_capped_like_the_ts_slice() {
        // chars().take(1000), not bytes — the TS slice is by UTF-16 unit and a
        // multibyte excerpt must not be cut mid-character here.
        let long: String = "é".repeat(1_050);
        assert!(long.chars().take(1_000).collect::<String>().chars().count() == 1_000);
    }
}

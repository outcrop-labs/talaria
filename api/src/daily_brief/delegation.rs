// Letting your assistant answer for you.
//
// THE LINE THIS DOES NOT CROSS. A delegated reply is posted with
// `author_type = 'agent'` under the assistant's own name — never as the owner.
// The person on the other end can see they were answered by Jon's assistant
// rather than by Jon, which is the difference between delegation and
// impersonation, and it is not a setting. `channel_messages` already models the
// distinction, so the honest version costs nothing; the dishonest version would
// have required writing the owner's handle into an author column on purpose.
//
// TWO WAYS TO SAY YES, and the default is the cautious one:
//
//   no grant    the assistant DRAFTS and stops. The draft rides on the brief's
//               conversation line, the owner approves or discards, and only an
//               approval posts it.
//   a grant     the assistant drafts and SENDS, on that thread or on all of
//               them, until the owner revokes it.
//
// Drafting freely and sending only on permission is the posture the workspace
// already takes with outbound Google actions — "reads and drafts are free;
// anything that leaves the building waits". A DM to a colleague leaves the
// building.
//
// WHY NOT `google_pending_actions`. It was the obvious reuse and it is the
// wrong home: that table resolves a Google OAuth token on approve, carries
// org-vs-personal audience rules built around a shared Google account, and is
// named for what it holds. A drafted reply also belongs ON the conversation
// line it answers rather than in a separate approvals list — the question
// "should this go?" is unanswerable without the message it is replying to next
// to it.
//
// STALENESS IS DERIVED, NEVER STORED. A draft names the message seq it answers.
// If the other person has said anything since, the draft is stale and the UI
// says so — approving it would post an answer to a question that has moved on.
// A stored `stale` flag would need something to notice and set it, and the
// thing it would be racing is the arrival of new messages.

use serde_json::json;
use sqlx::PgPool;

use crate::channels::insert_channel_message;
use crate::harness::defs::briefer::assistant_reply_harness;
use crate::harness::run::{RunContext, run_harness};
use crate::notify::NotifyDeps;
use crate::state::AppState;

/// May the assistant send in this conversation without asking?
pub async fn may_reply(pg: &PgPool, user_id: &str, channel_id: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from assistant_reply_grants \
         where user_id = $1::uuid and revoked_at is null \
           and (channel_id is null or channel_id = $2::uuid) limit 1",
    )
    .bind(user_id)
    .bind(channel_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

pub struct DraftRequest<'a> {
    pub user_id: &'a str,
    pub channel_id: &'a str,
    pub peer: &'a str,
    /// Seq of the message being answered — what makes the draft go stale.
    pub awaiting_seq: i32,
    pub agent_model: &'a str,
    pub owner_name: &'a str,
}

/// What a draft did: (sent?, draft id).
pub type Drafted = (bool, String);

/// Write a reply, and either send it or park it for approval.
///
/// Returns what happened, which the caller turns into the line's state. `None`
/// means the model produced nothing usable — the thread simply stays waiting,
/// which is the correct degradation: an unanswered message is a state the
/// surface already renders honestly. Errors are the caller's to log per-thread.
pub async fn draft_reply(
    state: &AppState,
    notify: &NotifyDeps,
    req: DraftRequest<'_>,
) -> Result<Option<Drafted>, String> {
    // Don't write a second draft over a live one. A person who has a draft
    // waiting for approval should not find it silently replaced on the next
    // sweep by a different one they never read — and if the thread has moved
    // on, the stale marker is how they find that out, not a rewrite.
    let open: Vec<(String, i32)> = sqlx::query_as(
        "select id::text, in_reply_to_seq from assistant_reply_drafts \
         where user_id = $1::uuid and channel_id = $2::uuid and status = 'pending'",
    )
    .bind(req.user_id)
    .bind(req.channel_id)
    .fetch_all(&state.pg)
    .await
    .map_err(|e| format!("open drafts read: {e}"))?;
    if open.iter().any(|(_, seq)| *seq >= req.awaiting_seq) {
        return Ok(None);
    }

    let history: Vec<(String, String, String)> = sqlx::query_as(
        "select author, author_type, content from channel_messages \
         where channel_id = $1::uuid and status = 'complete' and seq <= $2 \
         order by seq desc limit 12",
    )
    .bind(req.channel_id)
    .bind(req.awaiting_seq)
    .fetch_all(&state.pg)
    .await
    .map_err(|e| format!("history read: {e}"))?;

    // Oldest first — a transcript reads forward.
    let transcript: Vec<String> = history
        .iter()
        .rev()
        .map(|(author, _, content)| format!("{author}: {content}"))
        .collect();
    let run = run_harness(
        state,
        &assistant_reply_harness(),
        &json!({
            "peer": req.peer,
            "owner": req.owner_name,
            "transcript": transcript,
        }),
        RunContext {
            caller: "briefer:reply".into(),
            user_id: None,
            model: Some(req.agent_model.to_string()),
            step: None,
            tier: None,
            effort: None,
            ledger: None,
            deps: None,
        },
    )
    .await
    .map_err(|e| format!("reply harness: {e}"))?;
    let content = run
        .value
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if content.is_empty() {
        return Ok(None);
    }

    let allowed = may_reply(&state.pg, req.user_id, req.channel_id)
        .await
        .map_err(|e| format!("grant read: {e}"))?;

    let draft_id: (String,) = sqlx::query_as(
        "insert into assistant_reply_drafts \
           (user_id, channel_id, in_reply_to_seq, agent_model, content, status, delegated) \
         values ($1::uuid, $2::uuid, $3, $4, $5, $6, $7) returning id::text",
    )
    .bind(req.user_id)
    .bind(req.channel_id)
    .bind(req.awaiting_seq)
    .bind(req.agent_model)
    .bind(&content)
    .bind(if allowed { "sent" } else { "pending" })
    .bind(allowed)
    .fetch_one(&state.pg)
    .await
    .map_err(|e| format!("draft insert: {e}"))?;
    let draft_id = draft_id.0;
    if !allowed {
        return Ok(Some((false, draft_id)));
    }

    // Sending goes through `insert_channel_message`, which is where the agent
    // write door lives — so a delegated reply is scanned and redacted on the
    // same path as every other agent-authored message, rather than on a second
    // one written here that would drift.
    let message = insert_channel_message(
        notify,
        req.channel_id,
        "agent",
        req.agent_model,
        &content,
        "complete",
        &serde_json::json!([]),
        None,
    )
    .await
    .map_err(|e| format!("send: {e}"))?;
    sqlx::query("update assistant_reply_drafts set message_id = $2::uuid, decided_at = now() where id = $1::uuid")
        .bind(&draft_id)
        .bind(&message.id)
        .execute(&state.pg)
        .await
        .map_err(|e| format!("draft mark sent: {e}"))?;
    Ok(Some((true, draft_id)))
}

/// Send drafts that a NEW grant has just made sendable.
///
/// THE GAP THIS CLOSES. Granting on a thread that already had a parked draft did
/// nothing at all: the sweep skips a conversation that has a live draft (so it
/// does not redraft over one the owner is reading), so the newly-permitted reply
/// sat there until the other person happened to say something else. From the
/// owner's side that is a switch labelled "let my assistant reply here" that
/// visibly does nothing — the worst kind of control, because the natural
/// response is to click it again.
///
/// Granting permission to send a reply that is already written means sending it.
/// Stale drafts are left alone, for the same reason `decide_draft`
/// One parked draft the sweep may now send: (id, channel, content, model,
/// seq it answers, seq of the last thing THEY said).
type ParkedDraft = (String, String, String, Option<String>, i32, Option<i32>);

/// refuses them.
pub async fn release_drafts(
    notify: &NotifyDeps,
    user_id: &str,
    channel_id: Option<&str>,
) -> Result<usize, String> {
    let rows: Vec<ParkedDraft> = sqlx::query_as(
        "select d.id::text, d.channel_id::text, d.content, d.agent_model, d.in_reply_to_seq, \
                (select max(m.seq) from channel_messages m
                  where m.channel_id = d.channel_id and m.status = 'complete'
                    and not (m.author_type = 'user' and m.author = (select coalesce(email, name, 'user') from users where id = $1::uuid))
                    and not (m.author_type = 'agent' and m.author = d.agent_model)) \
         from assistant_reply_drafts d \
         where d.user_id = $1::uuid and d.status = 'pending' \
           and ($2::uuid is null or d.channel_id = $2::uuid) \
           and exists ( \
             select 1 from assistant_reply_grants g \
             where g.user_id = $1::uuid and g.revoked_at is null \
               and (g.channel_id is null or g.channel_id = d.channel_id) \
           )",
    )
    .bind(user_id)
    .bind(channel_id)
    .fetch_all(&notify.pg)
    .await
    .map_err(|e| format!("pending drafts read: {e}"))?;

    let mut sent = 0;
    for (id, channel, content, agent_model, seq, their_latest) in rows {
        if their_latest.is_some_and(|latest| latest > seq) {
            continue;
        }
        let message = insert_channel_message(
            notify,
            &channel,
            "agent",
            agent_model.as_deref().unwrap_or("assistant"),
            &content,
            "complete",
            &serde_json::json!([]),
            None,
        )
        .await
        .map_err(|e| format!("send: {e}"))?;
        sqlx::query(
            "update assistant_reply_drafts \
             set status = 'sent', delegated = true, message_id = $2::uuid, decided_at = now() \
             where id = $1::uuid",
        )
        .bind(&id)
        .bind(&message.id)
        .execute(&notify.pg)
        .await
        .map_err(|e| format!("draft mark sent: {e}"))?;
        sent += 1;
    }
    Ok(sent)
}

// ── The grant routes' engine ─────────────────────────────────────────────────

/// A live grant as the wire serves it (ReplyGrant) — camelCase, wire field
/// order. `grantedAt` is an ISO string with millisecond precision.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyGrant {
    pub id: String,
    /// Null = every conversation.
    pub channel_id: Option<String>,
    pub granted_at: String,
}

pub async fn list_grants(pg: &PgPool, user_id: &str) -> Result<Vec<ReplyGrant>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, f64)> = sqlx::query_as(
        "select id::text, channel_id::text, \
                (trunc(extract(epoch from granted_at) * 1000))::float8 \
         from assistant_reply_grants where user_id = $1::uuid and revoked_at is null \
         order by granted_at desc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, channel_id, granted_ms)| ReplyGrant {
            id,
            channel_id,
            granted_at: crate::agent_auth::epoch_ms_to_iso(granted_ms as i64),
        })
        .collect())
}

/// Grant reply authority. `channel_id: None` is the standing grant.
///
/// The owner's own decision, so the only authority check is that this is their
/// conversation — verified here rather than trusted from the route, because a
/// grant on somebody else's DM would let an agent speak in a room its owner
/// cannot even read. None IS that refusal: the route answers its 403.
pub async fn grant_reply(
    pg: &PgPool,
    user_id: &str,
    channel_id: Option<&str>,
) -> Result<Option<ReplyGrant>, sqlx::Error> {
    if let Some(channel_id) = channel_id {
        let member: Option<(i32,)> = sqlx::query_as(
            "select 1 from channel_members where channel_id = $1::uuid and user_id = $2::uuid",
        )
        .bind(channel_id)
        .bind(user_id)
        .fetch_optional(pg)
        .await?;
        if member.is_none() {
            return Ok(None);
        }
    }
    let granted: Option<(String, Option<String>, f64)> = sqlx::query_as(
        "insert into assistant_reply_grants (user_id, channel_id) values ($1::uuid, $2::uuid) \
         on conflict do nothing \
         returning id::text, channel_id::text, (trunc(extract(epoch from granted_at) * 1000))::float8",
    )
    .bind(user_id)
    .bind(channel_id)
    .fetch_optional(pg)
    .await?;
    if let Some((id, channel_id, granted_ms)) = granted {
        return Ok(Some(ReplyGrant {
            id,
            channel_id,
            granted_at: crate::agent_auth::epoch_ms_to_iso(granted_ms as i64),
        }));
    }
    // The partial unique indexes make a re-grant a no-op; return the live one
    // so the caller sees the state rather than a None it would read as
    // failure. `is not distinct from` — the standing grant matches on NULL.
    let existing: Option<(String, Option<String>, f64)> = sqlx::query_as(
        "select id::text, channel_id::text, (trunc(extract(epoch from granted_at) * 1000))::float8 \
         from assistant_reply_grants \
         where user_id = $1::uuid and revoked_at is null and channel_id is not distinct from $2::uuid",
    )
    .bind(user_id)
    .bind(channel_id)
    .fetch_optional(pg)
    .await?;
    Ok(existing.map(|(id, channel_id, granted_ms)| ReplyGrant {
        id,
        channel_id,
        granted_at: crate::agent_auth::epoch_ms_to_iso(granted_ms as i64),
    }))
}

/// Revoke it. Kept as a row with `revoked_at` — who was allowed to speak for
/// someone, and when that stopped, is exactly the history you want when a
/// reply turns out to have been wrong.
pub async fn revoke_reply(
    pg: &PgPool,
    user_id: &str,
    channel_id: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "update assistant_reply_grants set revoked_at = now() \
         where user_id = $1::uuid and revoked_at is null and channel_id is not distinct from $2::uuid \
         returning id::text",
    )
    .bind(user_id)
    .bind(channel_id)
    .fetch_all(pg)
    .await?;
    Ok(!rows.is_empty())
}

// ── Deciding a parked draft ──────────────────────────────────────────────────

/// What a decide did; the route maps each shape to its wire answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftOutcome {
    Sent,
    Rejected,
    /// They have said something since the draft was written.
    Stale(&'static str),
    /// Not found — or not the caller's; indistinguishable on purpose.
    Gone,
}

/// Approve or discard a parked reply.
///
/// A STALE DRAFT IS REFUSED RATHER THAN SENT, and this is the one branch worth
/// reading twice: between the draft being written and the owner clicking
/// approve, the other person may have said something else. Posting the old
/// answer then is worse than posting nothing, because it reads as a reply to
/// the newest message and is not one. The refusal is not an error state — the
/// caller re-drafts against what the thread now says.
///
/// Sending goes through `insert_channel_message`, which is where the agent
/// write door lives — a delegated reply is scanned and redacted on the same
/// path as every other agent-authored message, rather than on a second one
/// written here that would drift.
pub async fn decide_draft(
    notify: &NotifyDeps,
    user_id: &str,
    draft_id: &str,
    approve: bool,
) -> Result<DraftOutcome, String> {
    // Scoped to `user_id` in the query, so somebody else's draft is
    // indistinguishable from one that does not exist — which is the right
    // amount to disclose.
    let rows: Vec<ParkedDraft> = sqlx::query_as(
        "select d.id::text, d.channel_id::text, d.content, d.agent_model, d.in_reply_to_seq, \
                (select max(m.seq) from channel_messages m
                  where m.channel_id = d.channel_id and m.status = 'complete'
                    and not (m.author_type = 'user' and m.author = (select coalesce(email, name, 'user') from users where id = $1::uuid))
                    and not (m.author_type = 'agent' and m.author = d.agent_model)) \
         from assistant_reply_drafts d \
         where d.id = $2::uuid and d.user_id = $1::uuid and d.status = 'pending'",
    )
    .bind(user_id)
    .bind(draft_id)
    .fetch_all(&notify.pg)
    .await
    .map_err(|e| format!("draft read: {e}"))?;
    let Some((id, channel, content, agent_model, seq, their_latest)) = rows.into_iter().next()
    else {
        return Ok(DraftOutcome::Gone);
    };

    if !approve {
        sqlx::query(
            "update assistant_reply_drafts \
             set status = 'rejected', decided_at = now(), decided_by = $1::uuid \
             where id = $2::uuid",
        )
        .bind(user_id)
        .bind(&id)
        .execute(&notify.pg)
        .await
        .map_err(|e| format!("draft mark rejected: {e}"))?;
        return Ok(DraftOutcome::Rejected);
    }

    if their_latest.is_some_and(|latest| latest > seq) {
        return Ok(DraftOutcome::Stale(
            "They have said something since this was written, so it would answer the wrong message. Ask for a fresh draft.",
        ));
    }

    let message = insert_channel_message(
        notify,
        &channel,
        "agent",
        agent_model.as_deref().unwrap_or("assistant"),
        &content,
        "complete",
        &serde_json::json!([]),
        None,
    )
    .await
    .map_err(|e| format!("send: {e}"))?;
    sqlx::query(
        "update assistant_reply_drafts \
         set status = 'sent', message_id = $2::uuid, decided_at = now(), decided_by = $1::uuid \
         where id = $3::uuid",
    )
    .bind(user_id)
    .bind(&message.id)
    .bind(&id)
    .execute(&notify.pg)
    .await
    .map_err(|e| format!("draft mark sent: {e}"))?;
    Ok(DraftOutcome::Sent)
}

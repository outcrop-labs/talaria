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
// anything that leaves the building waits" (google/pending-actions.ts). A DM to
// a colleague leaves the building.
//
// WHY NOT `google_pending_actions`. It was the obvious reuse and it is the wrong
// home: that table resolves a Google OAuth token on approve, carries org-vs-
// personal audience rules built around a shared Google account, and is named for
// what it holds. A drafted reply also belongs ON the conversation line it
// answers rather than in a separate approvals list — the question "should this
// go?" is unanswerable without the message it is replying to next to it.
//
// STALENESS IS DERIVED, NEVER STORED. A draft names the message seq it answers.
// If the other person has said anything since, the draft is stale and the UI
// says so — approving it would post an answer to a question that has moved on.
// A stored `stale` flag would need something to notice and set it, and the
// thing it would be racing is the arrival of new messages.
import { db } from './db/pg'
import { insertChannelMessage } from './channels'
import { runHarness } from './harness/run'
import { assistantReplyHarness } from './harness/defs/briefer'
import { publishUser } from './realtime'

export interface ReplyGrant {
  id: string
  /** Null = every conversation. */
  channelId: string | null
  grantedAt: string
}

/** May the assistant send in this conversation without asking? */
export async function mayReply(userId: string, channelId: string): Promise<boolean> {
  const sql = await db()
  const rows = (await sql`
    select 1 from assistant_reply_grants
    where user_id = ${userId} and revoked_at is null
      and (channel_id is null or channel_id = ${channelId})
    limit 1
  `) as unknown as Array<{ '?column?': number }>
  return rows.length > 0
}

export async function listGrants(userId: string): Promise<ReplyGrant[]> {
  const sql = await db()
  return (await sql`
    select id, channel_id as "channelId", granted_at as "grantedAt"
    from assistant_reply_grants where user_id = ${userId} and revoked_at is null
    order by granted_at desc
  `) as unknown as ReplyGrant[]
}

/** Grant reply authority. `channelId: null` is the standing grant.
 *
 *  The owner's own decision, so the only authority check is that this is their
 *  conversation — verified here rather than trusted from the route, because a
 *  grant on somebody else's DM would let an agent speak in a room its owner
 *  cannot even read. */
export async function grantReply(userId: string, channelId: string | null): Promise<ReplyGrant | null> {
  const sql = await db()
  if (channelId) {
    const member = (await sql`
      select 1 from channel_members where channel_id = ${channelId} and user_id = ${userId}
    `) as unknown as unknown[]
    if (member.length === 0) return null
  }
  const rows = (await sql`
    insert into assistant_reply_grants (user_id, channel_id) values (${userId}, ${channelId})
    on conflict do nothing
    returning id, channel_id as "channelId", granted_at as "grantedAt"
  `) as unknown as ReplyGrant[]
  if (rows[0]) return rows[0]
  // The partial unique indexes make a re-grant a no-op; return the live one so
  // the caller sees the state rather than a null it would read as failure.
  const existing = (await sql`
    select id, channel_id as "channelId", granted_at as "grantedAt"
    from assistant_reply_grants
    where user_id = ${userId} and revoked_at is null and channel_id is not distinct from ${channelId}
  `) as unknown as ReplyGrant[]
  return existing[0] ?? null
}

/** Revoke it. Kept as a row with `revoked_at` — who was allowed to speak for
 *  someone, and when that stopped, is exactly the history you want when a reply
 *  turns out to have been wrong. */
export async function revokeReply(userId: string, channelId: string | null): Promise<boolean> {
  const sql = await db()
  const rows = (await sql`
    update assistant_reply_grants set revoked_at = now()
    where user_id = ${userId} and revoked_at is null and channel_id is not distinct from ${channelId}
    returning id
  `) as unknown as unknown[]
  return rows.length > 0
}

// ── Drafting ─────────────────────────────────────────────────────────────────

export interface DraftRequest {
  userId: string
  channelId: string
  peer: string
  /** Seq of the message being answered — what makes the draft go stale. */
  awaitingSeq: number
  agentModel: string
  ownerName: string
}

const HISTORY = 12

/** Write a reply, and either send it or park it for approval.
 *
 *  Returns what happened, which the caller turns into the line's state. `null`
 *  means the model produced nothing usable — the thread simply stays waiting,
 *  which is the correct degradation: an unanswered message is a state the
 *  surface already renders honestly. */
export async function draftReply(req: DraftRequest): Promise<{ status: 'sent' | 'pending'; draftId: string } | null> {
  const sql = await db()

  // Don't write a second draft over a live one. A person who has a draft waiting
  // for approval should not find it silently replaced on the next sweep by a
  // different one they never read — and if the thread has moved on, the stale
  // marker is how they find that out, not a rewrite.
  const open = (await sql`
    select id, in_reply_to_seq as "seq" from assistant_reply_drafts
    where user_id = ${req.userId} and channel_id = ${req.channelId} and status = 'pending'
  `) as unknown as Array<{ id: string; seq: number }>
  if (open.some((d) => d.seq >= req.awaitingSeq)) return null

  const history = (await sql`
    select author, author_type as "authorType", content
    from channel_messages
    where channel_id = ${req.channelId} and status = 'complete' and seq <= ${req.awaitingSeq}
    order by seq desc limit ${HISTORY}
  `) as unknown as Array<{ author: string; authorType: string; content: string }>

  const run = await runHarness(
    assistantReplyHarness,
    {
      peer: req.peer,
      owner: req.ownerName,
      transcript: [...history].reverse().map((m) => `${m.author}: ${m.content}`),
    },
    { caller: 'briefer:reply', model: req.agentModel },
  )
  const content = run.value?.trim()
  if (!content) return null

  const allowed = await mayReply(req.userId, req.channelId)

  const rows = (await sql`
    insert into assistant_reply_drafts
      (user_id, channel_id, in_reply_to_seq, agent_model, content, status, delegated)
    values (${req.userId}, ${req.channelId}, ${req.awaitingSeq}, ${req.agentModel}, ${content},
            ${allowed ? 'sent' : 'pending'}, ${allowed})
    returning id
  `) as unknown as Array<{ id: string }>
  const draftId = rows[0]!.id
  if (!allowed) return { status: 'pending', draftId }

  // Sending goes through `insertChannelMessage`, which is where the agent write
  // door lives — so a delegated reply is scanned and redacted on the same path
  // as every other agent-authored message, rather than on a second one written
  // here that would drift.
  const message = await insertChannelMessage(req.channelId, 'agent', req.agentModel, content)
  await sql`update assistant_reply_drafts set message_id = ${message.id}, decided_at = now() where id = ${draftId}`
  return { status: 'sent', draftId }
}

/** Send drafts that a NEW grant has just made sendable.
 *
 *  THE GAP THIS CLOSES. Granting on a thread that already had a parked draft did
 *  nothing at all: the sweep skips a conversation that has a live draft (so it
 *  does not redraft over one the owner is reading), so the newly-permitted reply
 *  sat there until the other person happened to say something else. From the
 *  owner's side that is a switch labelled "let my assistant reply here" that
 *  visibly does nothing — the worst kind of control, because the natural
 *  response is to click it again.
 *
 *  Granting permission to send a reply that is already written means sending it.
 *  Stale drafts are left alone, for the same reason `decideDraft` refuses them.
 *  Called from the sweep and from the grant route, so the toggle acts at once
 *  rather than at the next tick. */
export async function releaseDrafts(userId: string, channelId?: string): Promise<number> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.channel_id as "channelId", d.content, d.agent_model as "agentModel", d.in_reply_to_seq as "seq",
           (select max(m.seq) from channel_messages m
             where m.channel_id = d.channel_id and m.status = 'complete'
               and not (m.author_type = 'user' and m.author = (select coalesce(email, name, 'user') from users where id = ${userId}))
               and not (m.author_type = 'agent' and m.author = d.agent_model)) as "theirLatest"
    from assistant_reply_drafts d
    where d.user_id = ${userId} and d.status = 'pending'
      and (${channelId ?? null}::uuid is null or d.channel_id = ${channelId ?? null}::uuid)
      and exists (
        select 1 from assistant_reply_grants g
        where g.user_id = ${userId} and g.revoked_at is null
          and (g.channel_id is null or g.channel_id = d.channel_id)
      )
  `) as unknown as Array<{
    id: string
    channelId: string
    content: string
    agentModel: string | null
    seq: number
    theirLatest: number | null
  }>

  let sent = 0
  for (const draft of rows) {
    if (draft.theirLatest !== null && draft.theirLatest > draft.seq) continue
    const message = await insertChannelMessage(draft.channelId, 'agent', draft.agentModel ?? 'assistant', draft.content)
    await sql`
      update assistant_reply_drafts
      set status = 'sent', delegated = true, message_id = ${message.id}, decided_at = now()
      where id = ${draft.id}
    `
    sent++
  }
  return sent
}

// ── Deciding a parked draft ──────────────────────────────────────────────────

export type DraftDecision = 'approve' | 'reject'

export interface DraftOutcome {
  status: 'sent' | 'rejected' | 'stale' | 'gone'
  message?: string
}

/** Approve or discard a parked reply.
 *
 *  A STALE DRAFT IS REFUSED RATHER THAN SENT, and this is the one branch worth
 *  reading twice: between the draft being written and the owner clicking
 *  approve, the other person may have said something else. Posting the old
 *  answer then is worse than posting nothing, because it reads as a reply to
 *  the newest message and is not one. The refusal is not an error state — the
 *  caller re-drafts against what the thread now says. */
export async function decideDraft(userId: string, draftId: string, decision: DraftDecision): Promise<DraftOutcome> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.channel_id as "channelId", d.content, d.agent_model as "agentModel",
           d.in_reply_to_seq as "seq", d.status,
           (select max(m.seq) from channel_messages m
             where m.channel_id = d.channel_id and m.status = 'complete'
               and not (m.author_type = 'user' and m.author = (select coalesce(email, name, 'user') from users where id = ${userId}))
               and not (m.author_type = 'agent' and m.author = d.agent_model)) as "theirLatest"
    from assistant_reply_drafts d
    where d.id = ${draftId} and d.user_id = ${userId} and d.status = 'pending'
  `) as unknown as Array<{
    id: string
    channelId: string
    content: string
    agentModel: string | null
    seq: number
    theirLatest: number | null
  }>
  const draft = rows[0]
  // Scoped to `user_id` in the query, so somebody else's draft is indistinguishable
  // from one that does not exist — which is the right amount to disclose.
  if (!draft) return { status: 'gone' }

  if (decision === 'reject') {
    await sql`update assistant_reply_drafts set status = 'rejected', decided_at = now(), decided_by = ${userId} where id = ${draftId}`
    return { status: 'rejected' }
  }

  if (draft.theirLatest !== null && draft.theirLatest > draft.seq) {
    return {
      status: 'stale',
      message: 'They have said something since this was written, so it would answer the wrong message. Ask for a fresh draft.',
    }
  }

  const message = await insertChannelMessage(draft.channelId, 'agent', draft.agentModel ?? 'assistant', draft.content)
  await sql`
    update assistant_reply_drafts
    set status = 'sent', message_id = ${message.id}, decided_at = now(), decided_by = ${userId}
    where id = ${draftId}
  `
  return { status: 'sent' }
}

/** Nudge the owner's open brief so an approved reply shows as answered without
 *  waiting for the next sweep. Id-shaped like every other user event. */
export function nudgeBrief(userId: string, briefId: string, seq: number): void {
  publishUser(userId, { type: 'brief', briefId, seq })
}

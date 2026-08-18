// Who is still waiting on an answer from you.
//
// THE BUG THIS FILE IS. The brief's first version took its conversations from
// `inbox-focus-sources.channelItems`, which selects DMs with UNREAD messages —
// the right question for a queue, and the wrong one for a document about who is
// waiting. Opening a DM drops it out of that query, so the sweep saw the key
// vanish and appended a resolution: the brief told you Priya's question had been
// handled because you had glanced at it. Reading is not answering, and the
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
// does not do is pretend you wrote it: `answeredBy` carries which of the two it
// was, all the way to the words on the line.
import { db } from './db/pg'
import { fingerprint, keyOf } from './inbox-focus-policy'
import type { BriefBadge, BriefPriority } from './daily-brief-types'

export type AnsweredBy = 'you' | 'assistant' | null

export interface CommsLine {
  key: string
  channelId: string
  peer: string
  /** Messages you have not read. Zero does NOT mean answered. */
  unread: number
  /** Null while they are still waiting. */
  answeredBy: AnsweredBy
  answeredAt: string | null
  /** Seq of the last message from them — what a draft would be answering. */
  awaitingSeq: number
  excerpt: string
  updatedAt: string
  /** A pending draft the assistant has written for this thread, if any. */
  draft: { id: string; content: string; stale: boolean; createdAt: string } | null
  /** The assistant may send here without asking. */
  delegated: boolean
  statusLabel: string
  priority: BriefPriority
  badge: BriefBadge | null
  sourceFingerprint: string
}

interface Row {
  channelId: string
  peerName: string | null
  peerEmail: string | null
  updatedAt: string
  lastSeq: number
  lastAuthor: string | null
  lastAuthorType: string | null
  lastAt: string | null
  lastContent: string | null
  unread: number
  awaitingSeq: number | null
  awaitingContent: string | null
  awaitingAuthor: string | null
  draftId: string | null
  draftContent: string | null
  draftSeq: number | null
  draftCreatedAt: string | null
  delegated: boolean
}

/** Every DM this person is in that has ever been spoken in, with the state of
 *  its last message, its pending draft, and whether the assistant may answer.
 *
 *  ONE QUERY, because the four facts have to agree with each other. Read as
 *  four passes, a thread could be reported awaiting a reply by one and answered
 *  by another between statements — and the thing being decided is whether to
 *  put words in somebody's mouth. */
export async function commsLines(userId: string, assistantModel: string | null): Promise<CommsLine[]> {
  const sql = await db()
  const rows = (await sql`
    with me as (select id, coalesce(email, name, 'user') as handle from users where id = ${userId})
    select c.id as "channelId",
           pu.name as "peerName", pu.email as "peerEmail",
           c.updated_at as "updatedAt",
           last.seq as "lastSeq", last.author as "lastAuthor", last.author_type as "lastAuthorType",
           last.created_at as "lastAt", last.content as "lastContent",
           (select count(*)::int from channel_messages m
             where m.channel_id = c.id and m.seq > member.last_read_seq and m.status = 'complete'
               and not (m.author_type = 'user' and m.author = me.handle)) as unread,
           theirs.seq as "awaitingSeq", theirs.content as "awaitingContent", theirs.author as "awaitingAuthor",
           d.id as "draftId", d.content as "draftContent", d.in_reply_to_seq as "draftSeq",
           d.created_at as "draftCreatedAt",
           exists(
             select 1 from assistant_reply_grants g
             where g.user_id = ${userId} and g.revoked_at is null
               and (g.channel_id is null or g.channel_id = c.id)
           ) as delegated
    from channels c
    join channel_members member on member.channel_id = c.id and member.user_id = ${userId}
    cross join me
    left join channel_members peer on peer.channel_id = c.id and peer.user_id <> ${userId}
    left join users pu on pu.id = peer.user_id
    -- The last thing said, by anyone. This is what decides "answered".
    left join lateral (
      select m.seq, m.author, m.author_type, m.created_at, m.content
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
        and not (m.author_type = 'agent' and m.author = ${assistantModel})
      order by m.seq desc limit 1
    ) theirs on true
    left join lateral (
      select id, content, in_reply_to_seq, created_at
      from assistant_reply_drafts
      where user_id = ${userId} and channel_id = c.id and status = 'pending'
      order by created_at desc limit 1
    ) d on true
    where c.archived_at is null and c.kind = 'dm' and last.seq is not null
    order by c.updated_at desc limit 100
  `) as unknown as Row[]

  const out: CommsLine[] = []
  for (const row of rows) {
    const mineByHand = row.lastAuthorType === 'user' && row.lastAuthor !== null && !isPeer(row, row.lastAuthor)
    const mineByAgent = row.lastAuthorType === 'agent' && !!assistantModel && row.lastAuthor === assistantModel
    const answeredBy: AnsweredBy = mineByAgent ? 'assistant' : mineByHand ? 'you' : null
    const peer = row.peerName ?? row.peerEmail ?? row.awaitingAuthor ?? 'this conversation'

    // Never spoken to us at all — nothing is waiting and nothing was answered.
    if (row.awaitingSeq === null) continue

    const stale = row.draftSeq !== null && row.draftSeq < row.awaitingSeq
    const draft =
      row.draftId && row.draftContent !== null
        ? { id: row.draftId, content: row.draftContent, stale, createdAt: asIso(row.draftCreatedAt) }
        : null

    out.push({
      key: keyOf('channel', row.channelId),
      channelId: row.channelId,
      peer,
      unread: row.unread,
      answeredBy,
      answeredAt: answeredBy ? asIso(row.lastAt) : null,
      awaitingSeq: row.awaitingSeq,
      excerpt: (row.awaitingContent ?? '').slice(0, 1_000),
      updatedAt: asIso(row.updatedAt),
      draft,
      delegated: row.delegated,
      statusLabel: label(row.unread, answeredBy, draft),
      // A person waiting on a human answer is p1 whether or not it has been
      // read. It is NOT downgraded once read — "I saw it" is the state this
      // section exists to keep visible, not a reason to stop showing it.
      priority: answeredBy ? 'ok' : 'p1',
      badge: draft && !draft.stale ? { label: 'DRAFT READY', tone: 'accent' } : null,
      // The fingerprint carries every field the line RENDERS, so a thread that
      // moves from unread to read appends a change rather than sitting there
      // claiming an unread count that is gone.
      sourceFingerprint: fingerprint({
        u: row.unread,
        a: answeredBy,
        s: row.awaitingSeq,
        l: row.lastSeq,
        d: draft ? `${draft.id}:${draft.stale}` : null,
        g: row.delegated,
      }),
    })
  }
  return out
}

/** Is this author string the peer rather than the owner?
 *
 *  A DM has exactly two people, so anything authored by a `user` that is not the
 *  peer is the owner. Asked this way round because the owner's handle is
 *  `coalesce(email, name, 'user')` and a row written before they had an email
 *  can carry the older spelling — treating an unrecognised author as THEM (and
 *  so leaving the line open) is the safe direction to be wrong in. The unsafe
 *  direction closes a line nobody answered. */
function isPeer(row: Row, author: string): boolean {
  return author === row.peerEmail || author === row.peerName
}

function label(unread: number, answeredBy: AnsweredBy, draft: CommsLine['draft']): string {
  if (answeredBy === 'assistant') return 'ASSISTANT REPLIED'
  if (answeredBy === 'you') return 'YOU REPLIED'
  if (draft && !draft.stale) return unread > 0 ? `${unread} UNREAD · DRAFT READY` : 'READ · DRAFT READY'
  if (unread > 0) return `${unread} UNREAD`
  // THE STATE THE OLD SOURCE COULD NOT SEE. Everything above it existed before;
  // this line is the fix.
  return 'READ, NOT ANSWERED'
}

const asIso = (value: string | null): string => (value ? new Date(value).toISOString() : new Date(0).toISOString())

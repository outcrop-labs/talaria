// "Something happened that the brief is about" — the nudge, and nothing else.
//
// WHY IT IS ITS OWN MODULE. `channels.ts` has to be able to say this, and it
// cannot import `daily-brief.ts`: the brief imports the delegation layer, which
// imports `insertChannelMessage` from channels. Anything with a real dependency
// on the brief would close that cycle. This file imports the pool and the
// realtime publisher and nothing else, so it can be called from anywhere the
// event happens.
//
// WHAT IT DOES NOT DO IS SWEEP. A sweep is four scoped queries and up to one
// model call, and posting a message in a busy DM would fire one per message per
// participant. It clears the sweep throttle and rings the bell instead: an open
// page refetches, `/api/brief` sweeps on the way through because the throttle is
// now clear, and the person sees the line close. A page nobody is looking at
// costs one UPDATE and picks the change up on the next scheduled pass.
import { db } from './db/pg'
import { publishUser } from './realtime'

/** Mark these people's current brief as needing a sweep, and tell their open
 *  pages.
 *
 *  Bounded to the last 48 hours so a long-lived account does not have every
 *  brief it has ever had rewritten by one DM. Only today's is ever swept
 *  (`sweepBrief` loads by today's date), so older rows are noise either way —
 *  the window is about the size of the UPDATE, not about correctness. */
export async function markBriefStale(userIds: readonly string[]): Promise<void> {
  const ids = [...new Set(userIds)].filter(Boolean)
  if (ids.length === 0) return
  const sql = await db()
  const rows = (await sql`
    update daily_briefs set last_swept_at = null
    where user_id = any(${ids}) and created_at > now() - interval '48 hours'
    returning id, user_id as "userId", last_seq as "lastSeq"
  `) as unknown as Array<{ id: string; userId: string; lastSeq: number }>
  // Id-shaped, like every other user event: the payload says a brief moved and
  // the client re-reads through the ordinary route with the ordinary ACL.
  for (const row of rows) publishUser(row.userId, { type: 'brief', briefId: row.id, seq: row.lastSeq })
}

/** The members of a channel — who a message in it might be about.
 *
 *  Kept here rather than imported from `channels.ts` for the cycle reason in
 *  the header. It is one query and it is the only thing this module needs to
 *  know about a channel. */
export async function channelMemberIds(channelId: string): Promise<string[]> {
  const sql = await db()
  const rows = (await sql`
    select user_id as "userId" from channel_members where channel_id = ${channelId}
  `) as unknown as Array<{ userId: string }>
  return rows.map((r) => r.userId)
}

/** A message landed in a conversation. Detached on purpose: the person who sent
 *  it is waiting on the POST that wrote it, and their reply must not block on
 *  somebody else's brief bookkeeping. */
export function briefsFollowMessage(channelId: string): void {
  void channelMemberIds(channelId)
    .then((ids) => markBriefStale(ids))
    .catch((e: unknown) => console.error(`[daily-brief] could not follow a message in ${channelId}:`, e))
}

// Confirm-sends: outbound Google actions an agent drafted, held for the owner's
// approval. Reads and drafts are free; anything that leaves the building (an
// email, a calendar invite) waits here until a human approves — then it executes
// as that human's Google.

import { db } from '../db/pg'
import { createEvent, type CreateEventInput } from './calendar'
import { getAccessToken } from './connections'
import { sendMessage, type SendInput } from './gmail'

export type PendingKind = 'gmail_send' | 'calendar_create'

export interface PendingAction {
  id: string
  kind: PendingKind
  summary: string | null
  payload: unknown
  agentModel: string | null
  ownerUserId: string | null
  status: string
  createdAt: string
}

const COLS = `id, kind, summary, payload, agent_model as "agentModel", owner_user_id as "ownerUserId",
  status, created_at as "createdAt"`

/** Queue an agent-drafted outbound action for its owner to approve. */
export async function queueAction(input: {
  kind: PendingKind
  summary: string
  payload: SendInput | CreateEventInput
  agentModel: string
  ownerUserId: string
}): Promise<PendingAction> {
  const sql = await db()
  const [row] = await sql<PendingAction[]>`
    insert into google_pending_actions (kind, summary, payload, agent_model, owner_user_id)
    values (${input.kind}, ${input.summary}, ${JSON.stringify(input.payload)}, ${input.agentModel}, ${input.ownerUserId})
    returning ${sql.unsafe(COLS)}
  `
  return row!
}

/** A user's pending (awaiting-decision) actions. */
export async function listPending(ownerUserId: string): Promise<PendingAction[]> {
  const sql = await db()
  return sql<PendingAction[]>`
    select ${sql.unsafe(COLS)} from google_pending_actions
    where owner_user_id = ${ownerUserId} and status = 'pending'
    order by created_at desc
  `
}

/** Count of a user's pending actions (for a badge). */
export async function pendingCount(ownerUserId: string): Promise<number> {
  const sql = await db()
  const [row] = await sql<{ n: number }[]>`
    select count(*)::int as n from google_pending_actions where owner_user_id = ${ownerUserId} and status = 'pending'
  `
  return row?.n ?? 0
}

/** Approve → execute as the owner's Google, or reject → drop. Only the owner may
 *  decide their own actions. Returns the new status (+ any execution result). */
export async function decideAction(
  actionId: string,
  userId: string,
  decision: 'approve' | 'reject',
  nowMs: number,
): Promise<{ status: string; message?: string } | null> {
  const sql = await db()
  const [action] = await sql<{ kind: PendingKind; payload: unknown; ownerUserId: string | null; status: string }[]>`
    select kind, payload, owner_user_id as "ownerUserId", status
    from google_pending_actions where id = ${actionId}
  `
  if (!action) return null
  if (action.ownerUserId !== userId) return { status: 'forbidden' }
  if (action.status !== 'pending') return { status: action.status } // already decided

  if (decision === 'reject') {
    await sql`update google_pending_actions set status = 'rejected', decided_at = now(), decided_by = ${userId} where id = ${actionId}`
    return { status: 'rejected' }
  }

  // Approve → execute as the owner.
  const token = await getAccessToken(userId, nowMs)
  if (!token) return { status: 'not_connected', message: 'Reconnect Google to run this action.' }

  try {
    let result: unknown
    if (action.kind === 'gmail_send') result = await sendMessage(userId, nowMs, action.payload as SendInput)
    else if (action.kind === 'calendar_create') result = await createEvent(userId, nowMs, action.payload as CreateEventInput)
    else throw new Error(`unknown action kind: ${action.kind}`)

    await sql`
      update google_pending_actions
      set status = 'executed', result = ${JSON.stringify(result)}, decided_at = now(), decided_by = ${userId}
      where id = ${actionId}
    `
    return { status: 'executed' }
  } catch (err) {
    await sql`
      update google_pending_actions
      set status = 'failed', result = ${JSON.stringify({ error: (err as Error).message })}, decided_at = now(), decided_by = ${userId}
      where id = ${actionId}
    `
    return { status: 'failed', message: 'Google rejected the action.' }
  }
}

// Confirm-sends: outbound Google actions an agent drafted, held for approval.
// Reads and drafts are free; anything that leaves the building (an email, a
// calendar invite) waits here until a human approves — then it executes.
//
//   personal action (a personal assistant, bound to its owner) → the OWNER approves
//   org action (a general agent, on the shared org account)     → an ADMIN approves

import { announceApproval } from '../approvals'
import { db } from '../db/pg'
import { markBriefStale } from '../daily-brief-stale'
import { createEventWithToken, type CreateEventInput } from './calendar'
import { getAccessToken } from './connections'
import { sendMessageWithToken, type SendInput } from './gmail'
import { getOrgAccessToken, getOrgTargets } from './org-connection'

export type PendingKind = 'gmail_send' | 'calendar_create'

export interface PendingAction {
  id: string
  kind: PendingKind
  summary: string | null
  payload: unknown
  agentModel: string | null
  ownerUserId: string | null
  isOrg: boolean
  status: string
  createdAt: string
}

const COLS = `id, kind, summary, payload, agent_model as "agentModel", owner_user_id as "ownerUserId",
  is_org as "isOrg", status, created_at as "createdAt"`

/** Queue an agent-drafted outbound action. Personal actions carry an ownerUserId;
 *  org actions set isOrg (ownerUserId null) and are approved by an admin. */
export async function queueAction(input: {
  kind: PendingKind
  summary: string
  payload: SendInput | CreateEventInput
  agentModel: string
  ownerUserId: string | null
  isOrg?: boolean
}): Promise<PendingAction> {
  const sql = await db()
  const [row] = await sql<PendingAction[]>`
    insert into google_pending_actions (kind, summary, payload, agent_model, owner_user_id, is_org)
    values (${input.kind}, ${input.summary}, ${JSON.stringify(input.payload)}, ${input.agentModel}, ${input.ownerUserId}, ${input.isOrg ?? false})
    returning ${sql.unsafe(COLS)}
  `
  // The row IS an approval the instant it lands, and the agent that drafted it
  // is stopped in front of it. Until this call existed the first a human heard
  // was the approval sweep's next tick — up to five minutes of an agent waiting
  // on a decision that takes four seconds, and before the sweep existed, the
  // next morning's digest.
  //
  // Not awaited: this function is servicing an agent's request and must not
  // wait on notification writes to four admins, nor fail because one of them
  // failed. Idempotent against the sweep — both mark `approval_announce_state`
  // by key, and the mark is merged in the database rather than written over the
  // blob, so this is latency removed and never a second notification.
  void announceApproval(`google_action:${row!.id}`).catch((e: unknown) =>
    console.error(`[google] queued action ${row!.id} could not be announced — the approval sweep will pick it up:`, e),
  )
  return row!
}

/** The actions a user should decide: their own personal ones, plus — for an
 *  admin — the org-scoped ones. */
export async function listPending(userId: string, isAdmin: boolean): Promise<PendingAction[]> {
  const sql = await db()
  return sql<PendingAction[]>`
    select ${sql.unsafe(COLS)} from google_pending_actions
    where status = 'pending'
      and ((is_org = false and owner_user_id = ${userId}) ${isAdmin ? sql`or is_org = true` : sql``})
    order by created_at desc
  `
}

/** Approve → execute (as the owner, or the org for org actions), or reject → drop.
 *  Personal actions: only the owner decides. Org actions: only an admin. */
export async function decideAction(
  actionId: string,
  actor: { id: string; isAdmin: boolean },
  decision: 'approve' | 'reject',
  nowMs: number,
): Promise<{ status: string; message?: string } | null> {
  const sql = await db()
  const [action] = await sql<{ kind: PendingKind; payload: unknown; ownerUserId: string | null; isOrg: boolean; status: string }[]>`
    select kind, payload, owner_user_id as "ownerUserId", is_org as "isOrg", status
    from google_pending_actions where id = ${actionId}
  `
  if (!action) return null
  const authorized = action.isOrg ? actor.isAdmin : action.ownerUserId === actor.id
  if (!authorized) return { status: 'forbidden' }
  if (action.status !== 'pending') return { status: action.status } // already decided

  // A terminal decision resolves the approval line on the owner's (and the
  // decider's) brief. Detached and silent: the decider is waiting on this
  // response. Org actions decided by one admin leave other admins' briefs to
  // the scheduled sweep — the nudge is an optimization, never the floor.
  const nudgeBrief = (): void => {
    void markBriefStale([action.ownerUserId, actor.id].filter((id): id is string => !!id)).catch(() => {})
  }

  if (decision === 'reject') {
    await sql`update google_pending_actions set status = 'rejected', decided_at = now(), decided_by = ${actor.id} where id = ${actionId}`
    nudgeBrief()
    return { status: 'rejected' }
  }

  // Approve → resolve the executing token (org account, or the owner's).
  const token = action.isOrg ? await getOrgAccessToken(nowMs) : await getAccessToken(action.ownerUserId!, nowMs)
  if (!token) {
    return { status: 'not_connected', message: action.isOrg ? 'Reconnect the org Google account to run this.' : 'Reconnect Google to run this action.' }
  }

  // Org actions land on the configured shared targets (calendar / send-as alias).
  const targets = action.isOrg ? await getOrgTargets() : null

  try {
    let result: unknown
    if (action.kind === 'gmail_send') result = await sendMessageWithToken(token, action.payload as SendInput, targets?.sendAs)
    else if (action.kind === 'calendar_create') result = await createEventWithToken(token, action.payload as CreateEventInput, targets?.calendarId)
    else throw new Error(`unknown action kind: ${action.kind}`)

    await sql`
      update google_pending_actions
      set status = 'executed', result = ${JSON.stringify(result)}, decided_at = now(), decided_by = ${actor.id}
      where id = ${actionId}
    `
    nudgeBrief()
    return { status: 'executed' }
  } catch (err) {
    await sql`
      update google_pending_actions
      set status = 'failed', result = ${JSON.stringify({ error: (err as Error).message })}, decided_at = now(), decided_by = ${actor.id}
      where id = ${actionId}
    `
    nudgeBrief()
    return { status: 'failed', message: 'Google rejected the action.' }
  }
}

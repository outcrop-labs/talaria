import { db } from './db/pg'
import { listPending } from './google/pending-actions'
import { statusCategorySql } from './statuses'
import type { SessionUser } from './api-guard'
import {
  ACTIONABLE_NOTIFICATION_KINDS,
  asIso,
  finalizeItem,
  focusAction,
  keyOf,
  nullableIso,
  priorityForBucket,
  taskBucket,
  taskQuestion,
  taskRecommendation,
  taskStatusLabel,
} from './inbox-focus-policy'
import type { RawFocusItem } from './inbox-focus-types'

export async function taskItems(userId: string, sourceId?: string): Promise<RawFocusItem[]> {
  const sql = await db()
  const rows = (await sql`
    select t.id, t.board_id as "boardId", b.name as board,
           case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end as "ticketRef",
           t.title, t.description, t.status, t.priority, t.due_date as "dueAt",
           t.error_message as "errorMessage", t.outcome,
           t.created_at as "createdAt", t.updated_at as "updatedAt",
           ${sql.unsafe(statusCategorySql('review', ['quality_review']))} as "isReview",
           (
             t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start)
             or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status = 'inbox')
           ) as "isTriage"
    from tasks t
    join boards b on b.id = t.board_id
    left join board_members m on m.board_id = b.id and m.user_id = ${userId}
    left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    where (m.user_id is not null or tm.user_id is not null)
      and b.archived_at is null and t.archived_at is null
      and (${sourceId ?? null}::text is null or t.id::text = ${sourceId ?? null})
      and (
        t.status in ('failed', 'blocked')
        or ${sql.unsafe(statusCategorySql('review', ['quality_review']))}
        or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start)
        or (
          not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
          and t.status in ('inbox', 'quality_review')
        )
      )
    order by t.created_at asc limit 200
  `) as unknown as Array<{
    id: string
    boardId: string
    board: string
    ticketRef: string | null
    title: string
    description: string | null
    status: string
    priority: string
    dueAt: string | null
    errorMessage: string | null
    outcome: string | null
    createdAt: string
    updatedAt: string
    isReview: boolean
    isTriage: boolean
  }>

  return rows.map((row) => {
    const bucket = taskBucket(row.status, row.isReview, row.isTriage)
    // `risk` DESCRIBES A HUMAN CLICK ON THIS CARD, and 'safe' is still right for
    // both: the reviewer is looking at the evidence, and asking them to confirm
    // their own click would be a modal for its own sake.
    //
    // It is NOT a statement that anything may run these without a person. That
    // reading is what made this line the sharpest edge in the Inbox — a widened
    // model can now select `approve_task`, and `confirmationRequired: false`
    // sent it straight to `completeQualityReview(..., 'approved')` with an audit
    // row naming the human who never clicked. Confirmation now depends on the
    // action AND on who proposed it (`requiresHumanConfirmation` in
    // inbox-focus-policy.ts), so a model-proposed sign-off takes the
    // confirmation path while a human's own click does not.
    const actions = row.isReview
      ? [
          focusAction('approve_task', 'Approve', 'safe'),
          focusAction('request_changes', 'Request changes', 'safe'),
        ]
      : []
    const fallbackAction = row.isReview ? 'approve_task' : null
    const base: Omit<RawFocusItem, 'sourceFingerprint'> = {
      key: keyOf('task', row.id),
      sourceType: 'task',
      sourceId: row.id,
      priority: priorityForBucket(bucket),
      statusLabel: taskStatusLabel(row.status, row.isReview, row.isTriage),
      createdAt: asIso(row.createdAt),
      updatedAt: asIso(row.updatedAt),
      dueAt: nullableIso(row.dueAt),
      question: taskQuestion(row.title, row.status, row.isReview, row.isTriage),
      recommendation: taskRecommendation(row.status, row.isReview),
      recommendedActionId: fallbackAction,
      evidence: [
        ...(row.description ? [{ label: 'Task', text: row.description.slice(0, 1_000) }] : []),
        ...(row.errorMessage ? [{ label: 'Failure', text: row.errorMessage.slice(0, 1_000) }] : []),
        ...(row.outcome ? [{ label: 'Outcome', text: row.outcome.slice(0, 1_000) }] : []),
      ],
      metadata: {
        board: row.board,
        ticket: row.ticketRef,
        status: row.status,
        priority: row.priority,
      },
      sourceHref: `/boards/${row.boardId}/${row.id}`,
      briefStatus: 'fallback',
      actions,
      bucket,
    }
    return finalizeItem(base)
  })
}

export async function approvalItems(user: SessionUser, sourceId?: string): Promise<RawFocusItem[]> {
  const pending = await listPending(user.id, user.role === 'admin')
  const approvals = sourceId ? pending.filter((approval) => approval.id === sourceId) : pending
  return approvals.map((approval) => {
    const label = approval.kind === 'gmail_send' ? 'SEND EMAIL' : 'CREATE EVENT'
    const base: Omit<RawFocusItem, 'sourceFingerprint'> = {
      key: keyOf('approval', approval.id),
      sourceType: 'approval',
      sourceId: approval.id,
      priority: 'p0',
      statusLabel: `APPROVAL · ${label}`,
      createdAt: asIso(approval.createdAt),
      updatedAt: asIso(approval.createdAt),
      dueAt: null,
      question: `${approval.kind === 'gmail_send' ? 'Send' : 'Create'} “${approval.summary ?? label.toLowerCase()}”?`,
      recommendation: 'Review the exact outbound payload before confirming this identity-bearing action.',
      recommendedActionId: 'approve',
      evidence: [
        { label: 'Draft', text: approval.summary ?? label.replaceAll('_', ' ').toLowerCase() },
        { label: 'Actor', text: approval.agentModel ?? 'Agent' },
      ],
      metadata: { kind: approval.kind, agent: approval.agentModel, organizationAction: approval.isOrg },
      sourceHref: '/',
      briefStatus: 'fallback',
      actions: [
        // Asymmetric on purpose: approving SENDS something under the owner's
        // identity, rejecting only declines to. Same note as the review pair
        // above — 'safe' is about a human's click, and a model that proposes
        // `reject` still takes the confirmation path.
        focusAction('approve', approval.kind === 'gmail_send' ? 'Approve send' : 'Approve event', 'confirmation'),
        focusAction('reject', 'Reject', 'safe'),
      ],
      bucket: 0,
    }
    return finalizeItem(base)
  })
}

export async function channelItems(userId: string, sourceId?: string): Promise<RawFocusItem[]> {
  const sql = await db()
  const rows = (await sql`
    select c.id, c.name, c.kind, c.created_at as "createdAt", c.updated_at as "updatedAt", c.msg_seq as "maxSeq",
           pu.name as "peerName", pu.email as "peerEmail",
           (select count(*)::int from channel_messages msg
             where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete'
               and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))
           ) as "unreadCount",
           (select msg.content from channel_messages msg
             where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete'
               and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))
             order by msg.seq desc limit 1
           ) as excerpt,
           (select msg.author from channel_messages msg
             where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete'
               and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))
             order by msg.seq desc limit 1
           ) as author
    from channels c
    join channel_members member on member.channel_id = c.id and member.user_id = ${userId}
    join users self on self.id = ${userId}
    left join channel_members peer on c.kind = 'dm' and peer.channel_id = c.id and peer.user_id <> ${userId}
    left join users pu on pu.id = peer.user_id
    where c.archived_at is null and c.kind = 'dm'
      and (${sourceId ?? null}::text is null or c.id::text = ${sourceId ?? null})
      and exists (
        select 1 from channel_messages msg
        where msg.channel_id = c.id and msg.seq > member.last_read_seq and msg.status = 'complete'
          and not (msg.author_type = 'user' and msg.author = coalesce(self.email, self.name, 'user'))
      )
    order by c.updated_at asc limit 100
  `) as unknown as Array<{
    id: string
    name: string
    kind: string
    peerName: string | null
    peerEmail: string | null
    maxSeq: number
    unreadCount: number
    excerpt: string | null
    author: string | null
    createdAt: string
    updatedAt: string
  }>

  return rows.map((row) => {
    const peer = row.peerName ?? row.peerEmail ?? row.author ?? 'this conversation'
    const base: Omit<RawFocusItem, 'sourceFingerprint'> = {
      key: keyOf('channel', row.id),
      sourceType: 'channel',
      sourceId: row.id,
      priority: 'p1',
      statusLabel: `DIRECT · ${row.unreadCount} UNREAD`,
      createdAt: asIso(row.createdAt),
      updatedAt: asIso(row.updatedAt),
      dueAt: null,
      question: `Reply to ${peer}?`,
      recommendation: 'Read the latest message, then reply or mark the conversation read.',
      recommendedActionId: 'reply',
      evidence: [{ label: row.author ?? peer, text: (row.excerpt ?? 'Unread direct message').slice(0, 1_000) }],
      metadata: { peer, unread: row.unreadCount, maxSeq: row.maxSeq },
      sourceHref: `/comms/channel/${row.id}`,
      briefStatus: 'fallback',
      actions: [
        focusAction('reply', 'Reply', 'confirmation'),
        focusAction('mark_read', 'Mark read', 'reversible'),
      ],
      bucket: 2,
    }
    return finalizeItem(base)
  })
}

export async function notificationItems(userId: string, sourceId?: string): Promise<RawFocusItem[]> {
  const sql = await db()
  const rows = (await sql`
    select id, kind, title, body, href, created_at as "createdAt"
    from notifications
    where user_id = ${userId} and read_at is null and kind = any(${[...ACTIONABLE_NOTIFICATION_KINDS]})
      and (${sourceId ?? null}::text is null or id::text = ${sourceId ?? null})
    order by created_at asc limit 200
  `) as unknown as Array<{
    id: string
    kind: string
    title: string
    body: string
    href: string
    createdAt: string
  }>

  return rows.map((row) => {
    const direct = ['mention', 'dm', 'agent-outreach'].includes(row.kind)
    const bucket = direct ? 2 : 5
    const base: Omit<RawFocusItem, 'sourceFingerprint'> = {
      key: keyOf('notification', row.id),
      sourceType: 'notification',
      sourceId: row.id,
      priority: priorityForBucket(bucket),
      statusLabel: row.kind.replaceAll('-', ' ').toUpperCase(),
      createdAt: asIso(row.createdAt),
      updatedAt: asIso(row.createdAt),
      dueAt: null,
      question: `Review “${row.title}”?`,
      recommendation: direct ? 'Open the source and respond if needed.' : 'Review the source, then mark this item read.',
      recommendedActionId: 'mark_read',
      evidence: [{ label: 'Source', text: (row.body || row.title).slice(0, 1_000) }],
      metadata: { kind: row.kind },
      sourceHref: row.href || '/',
      briefStatus: 'fallback',
      actions: [focusAction('mark_read', 'Mark read', 'reversible')],
      bucket,
    }
    return finalizeItem(base)
  })
}

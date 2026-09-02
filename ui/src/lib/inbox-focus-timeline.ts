import type {
  InboxFocusContext,
  InboxMessageMetadata,
  InboxTimelineEntry,
} from './inbox-focus-types'

export type InboxTimelineRecord =
  | {
      recordType: 'message'
      id: string
      createdAt: string | Date
      role: 'user' | 'assistant'
      content: string
      status: 'streaming' | 'complete' | 'error'
      metadata: InboxMessageMetadata | null
      attachments?: Array<{ id: string; filename: string; mime: string; size: number; refType?: 'kb-doc' | 'artifact' }>
    }
  | {
      recordType: 'decision'
      id: string
      createdAt: string | Date
      status: string
      actionId: string | null
      instruction: string | null
      proposal: unknown
      outcome: unknown
      focus: InboxFocusContext
      confirmationToken?: string
      expiresAt?: string
      undoExpiresAt?: string
    }

function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null
}

export function normalizeInboxTimelineTimestamp(value: string | Date): string {
  return value instanceof Date ? value.toISOString() : value
}

function focusFromMetadata(metadata: InboxMessageMetadata | null): InboxFocusContext | null {
  const focus = object(metadata?.focus)
  if (!focus || typeof focus.key !== 'string' || typeof focus.question !== 'string' || typeof focus.sourceHref !== 'string') return null
  return focus as unknown as InboxFocusContext
}

function activityFor(record: Extract<InboxTimelineRecord, { recordType: 'decision' }>): InboxTimelineEntry {
  const proposal = object(record.proposal)
  const outcome = object(record.outcome)
  const preview = object(proposal?.preview)
  const undone = outcome?.undone === true
  const activity = undone
    ? 'undo'
    : record.status === 'confirmed'
      ? 'failure'
    : record.status === 'failed'
      ? 'failure'
      : record.status === 'cancelled'
        ? 'cancellation'
        : record.status === 'completed'
          ? 'completion'
          : record.confirmationToken
            ? 'confirmation'
            : 'proposal'
  const message = typeof proposal?.message === 'string'
    ? proposal.message
    : typeof outcome?.message === 'string'
      ? outcome.message
      : typeof outcome?.error === 'string'
        ? outcome.error
        : record.status === 'confirmed'
          ? 'Confirmation was accepted, but the final outcome could not be verified. Open the source before retrying.'
        : null
  const entry: InboxTimelineEntry = {
    kind: 'activity',
    id: `decision:${record.id}`,
    createdAt: normalizeInboxTimelineTimestamp(record.createdAt),
    activity,
    decisionId: record.id,
    focus: record.focus,
    actionId: record.actionId,
    title: typeof preview?.title === 'string' ? preview.title : record.focus.question,
    details: preview?.details ?? outcome ?? proposal?.payload ?? null,
    message,
  }
  if (record.confirmationToken) entry.confirmationToken = record.confirmationToken
  if (record.expiresAt) entry.expiresAt = record.expiresAt
  if (record.undoExpiresAt) entry.undoExpiresAt = record.undoExpiresAt
  return entry
}

export function buildInboxTimeline(records: InboxTimelineRecord[]): InboxTimelineEntry[] {
  const ordered = [...records].sort((a, b) => (
    normalizeInboxTimelineTimestamp(a.createdAt).localeCompare(normalizeInboxTimelineTimestamp(b.createdAt))
    || a.id.localeCompare(b.id)
  ))
  const entries: InboxTimelineEntry[] = []
  let lastFocusKey: string | null = null
  for (const record of ordered) {
    if (
      record.recordType === 'decision'
      && record.actionId === null
      && object(record.outcome)?.kind === 'clarification'
    ) continue
    const focus = record.recordType === 'message' ? focusFromMetadata(record.metadata) : record.focus
    if (focus && focus.key !== lastFocusKey) {
      entries.push({
        kind: 'context',
        id: `context:${record.id}`,
        createdAt: normalizeInboxTimelineTimestamp(record.createdAt),
        focus,
      })
      lastFocusKey = focus.key
    } else if (!focus) {
      lastFocusKey = null
    }
    if (record.recordType === 'message') {
      entries.push({
        kind: 'message',
        id: record.id,
        role: record.role,
        content: record.content,
        status: record.status,
        createdAt: normalizeInboxTimelineTimestamp(record.createdAt),
        focus,
        actor: record.metadata?.actor ?? null,
        delegateModel: record.metadata?.delegateModel ?? null,
        responseModel: record.metadata?.responseModel ?? null,
        mode: record.metadata?.mode ?? null,
        attachments: record.attachments ?? [],
      })
    } else {
      entries.push(activityFor(record))
    }
  }
  return entries
}

export function mergeInboxTimelinePages(pages: InboxTimelineEntry[][]): InboxTimelineEntry[] {
  const seen = new Set<string>()
  const merged: InboxTimelineEntry[] = []
  let lastFocusKey: string | null = null
  for (const entry of [...pages].reverse().flat()) {
    if (seen.has(entry.id)) continue
    seen.add(entry.id)
    if (entry.kind === 'context') {
      if (entry.focus.key === lastFocusKey) continue
      lastFocusKey = entry.focus.key
      merged.push(entry)
      continue
    }
    lastFocusKey = entry.focus?.key ?? null
    merged.push(entry)
  }
  return merged
}

export function encodeInboxTimelineCursor(createdAt: string, id: string): string {
  return encodeURIComponent(`${createdAt}|${id}`)
}

export function decodeInboxTimelineCursor(cursor: string): { createdAt: string; id: string } | null {
  try {
    const decoded = decodeURIComponent(cursor)
    const split = decoded.lastIndexOf('|')
    if (split <= 0) return null
    const createdAt = decoded.slice(0, split)
    const id = decoded.slice(split + 1)
    if (!id || Number.isNaN(Date.parse(createdAt))) return null
    return { createdAt, id }
  } catch {
    return null
  }
}

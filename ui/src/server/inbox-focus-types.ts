export const FOCUS_SOURCE_TYPES = ['approval', 'task', 'channel', 'notification'] as const
export type FocusSourceType = (typeof FOCUS_SOURCE_TYPES)[number]
export type FocusPriority = 'p0' | 'p1' | 'p2' | 'ok'
export type FocusBriefStatus = 'cached' | 'pending' | 'fallback'
export type FocusActionRisk = 'safe' | 'reversible' | 'confirmation'

export interface FocusEvidence {
  label: string
  text: string
}

export interface FocusAction {
  id: string
  label: string
  risk: FocusActionRisk
  confirmationRequired: boolean
  reversible: boolean
}

export interface FocusItem {
  key: string
  sourceType: FocusSourceType
  sourceId: string
  priority: FocusPriority
  statusLabel: string
  createdAt: string
  updatedAt: string
  dueAt: string | null
  question: string
  recommendation: string
  recommendedActionId: string | null
  evidence: FocusEvidence[]
  metadata: Record<string, string | number | boolean | null>
  sourceHref: string
  briefStatus: FocusBriefStatus
  actions: FocusAction[]
}

export interface InboxFocusContext {
  key: string
  question: string
  sourceHref: string
  sourceType?: FocusSourceType
  sourceId?: string
}

export interface InboxMessageMetadata {
  focus?: InboxFocusContext | null
  actor?: { type: 'human' | 'assistant'; id: string | null; label: string }
  delegateModel?: string | null
  decisionId?: string | null
}

export type InboxActivityStatus =
  | 'proposal'
  | 'confirmation'
  | 'completion'
  | 'failure'
  | 'cancellation'
  | 'undo'

export type InboxTimelineEntry =
  | {
      kind: 'message'
      id: string
      role: 'user' | 'assistant'
      content: string
      status: 'streaming' | 'complete' | 'error'
      createdAt: string
      focus: InboxFocusContext | null
      actor: InboxMessageMetadata['actor'] | null
      delegateModel: string | null
    }
  | {
      kind: 'context'
      id: string
      createdAt: string
      focus: InboxFocusContext
    }
  | {
      kind: 'activity'
      id: string
      createdAt: string
      activity: InboxActivityStatus
      decisionId: string
      focus: InboxFocusContext
      actionId: string | null
      title: string
      details: unknown
      message: string | null
      confirmationToken?: string
      expiresAt?: string
      undoExpiresAt?: string
    }

export interface InboxConversationPage {
  conversationId: string
  entries: InboxTimelineEntry[]
  nextCursor: string | null
  working: boolean
}

export type InboxCommandEvent =
  | { type: 'conversation'; conversationId: string; entry: InboxTimelineEntry }
  | { type: 'status'; label: string }
  | { type: 'content'; text: string }
  | { type: 'activity'; entry: InboxTimelineEntry }
  | { type: 'done'; conversationId: string; entry: InboxTimelineEntry; result?: FocusCommandResponse }
  | { type: 'error'; message: string }

export interface RawFocusItem extends FocusItem {
  bucket: number
  sourceFingerprint: string
}

export interface FocusCounts {
  total: number
  approvals: number
  tasks: number
  comms: number
  notifications: number
}

export interface FocusAssistant {
  configured: boolean
  model: string | null
  name: string | null
}

export interface FocusQueue {
  items: FocusItem[]
  counts: FocusCounts
  assistant: FocusAssistant
}

export interface FocusActionResult {
  status: 'completed' | 'confirmation_required' | 'cancelled' | 'undone' | 'stale' | 'failed'
  message?: string
  decisionId?: string
  confirmationToken?: string
  expiresAt?: string
  preview?: { title: string; details: unknown }
  undo?: { decisionId: string; expiresAt: string }
  result?: unknown
  timelineEntry?: InboxTimelineEntry
}

export interface FocusCommandResult {
  kind: 'clarification' | 'proposal'
  message: string
  actionId?: string
  payload?: Record<string, unknown>
  decisionId: string
  assistant: FocusAssistant
  consultedModel?: string | null
}

export interface FocusCommandStale {
  status: 'stale'
  message: string
}

export type FocusCommandResponse = FocusCommandResult | FocusCommandStale

export interface AssistantBrief {
  question: string
  recommendation: string
  recommendedActionId: string | null
}

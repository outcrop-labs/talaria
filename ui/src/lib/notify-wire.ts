// The notification contract's wire, as the client sees it (docs/notifications/
// contract.md, I-10 … I-24). TYPES AND CONSTS ONLY — framework-free, so the
// page, the service worker, the shell bridge and the tests all import it with
// no runtime cost. The server (talaria-notify, talaria-realtime, talaria-push)
// is the source; this file is its mirror, pinned by notify-wire.test.ts
// against JSON captured from a running api.
//
// Every field here is ADDITIVE to wave 1's: an api that predates the contract
// leaves them absent, so a consumer reads each one as optional and gates new
// behaviour on `features` (I-23), never on a version number.

import { ARRIVALS_KEY } from './notify-arrivals'

export { ARRIVALS_KEY }

// ── I-12: the class registry ─────────────────────────────────────────────────

/** Where a class sits: For you = needs_you + direct. */
export type Category = 'needs_you' | 'direct' | 'following' | 'workspace'

/** One class's delivery channels. In-app is always on — the row is the record. */
export interface Channels {
  inApp: true
  desktop: boolean
  push: boolean
  email: boolean
}

/** The registry's defaults for a class (In-app is implied). */
export type ChannelDefaults = Omit<Channels, 'inApp'>

/** One entry of `classes[]`, in registry (wire) order. */
export interface ClassInfo {
  id: string
  label: string
  blurb: string
  category: Category
  defaults: ChannelDefaults
  /** Alerts through DND: a decision a person is blocking. */
  breaksThrough: boolean
  /** Members are never served one. */
  adminOnly: boolean
  /** Minutes the missed-activity mail waits; 0 = the person's frequency. */
  emailDelayMin: number
  /** The kinds that land in it, with the reason each shows. */
  kinds: Array<{ id: string; reason: string }>
}

// ── I-13 / I-14: prefs v2, DND, frequency ────────────────────────────────────

/** `channels`: class id → its effective channels. */
export type ChannelsPrefs = Record<string, Channels>

/** A PATCH to the matrix: class id → the bools to change. `inApp: false` is
 *  refused (400 "In-app is always on"). */
export type ChannelsPatch = Record<string, Partial<Omit<Channels, 'inApp'>>>

/** Working hours: inside them notifications alert, outside they are quiet.
 *  `days`: 0 = Sunday. `start > end` crosses midnight. */
export interface DndSchedule {
  enabled: boolean
  days: number[]
  start: string
  end: string
}

/** What is stored. `pausedUntil` is an ISO instant, `FOREVER` for "until I
 *  resume". */
export interface DndSettings {
  pausedUntil: string | null
  schedule: DndSchedule | null
  allowNeedsYou: boolean
}

/** What it means now, computed by the server in the person's zone (the one
 *  evaluator, `dnd_state`). */
export interface DndState {
  active: boolean
  until: string | null
  reason: 'paused' | 'schedule' | null
  timeZone: string
}

export type DndWire = DndSettings & DndState

export type EmailFrequency = '15m' | '1h' | '1d' | 'off'

/** "Until I resume". */
export const FOREVER = '9999-12-31T23:59:59.999Z'

// ── I-23: features ───────────────────────────────────────────────────────────

/** The flags an api serves in `features`. Phase 0's are listed; each wave-2
 *  stream adds its own (a string the client does not know is ignored). */
export type NotifyFeature =
  | 'seq'
  | 'classes'
  | 'channels'
  | 'dnd-store'
  | 'presence-store'
  | 'ack'
  | 'devices'
  | 'read-ids'
  | 'channel-levels-store'
  | (string & {})

// ── I-11: the row, v2 ────────────────────────────────────────────────────────

/** How a client presents an arrival (I-15). A `feed` row is recorded and never
 *  toasted or bannered. */
export type Alert = 'banner' | 'feed'

export interface NotificationActor {
  kind: 'user' | 'agent' | 'system'
  id: string | null
  name: string | null
}

export interface NotificationEntity {
  type: string
  id: string | null
}

/** A row as `GET /api/notifications` serves it since the contract: wave 1's
 *  seven keys, then the v2 ones. `seq` is null on a row filed before it. */
export interface NotificationRowV2 {
  id: string
  kind: string
  title: string
  body: string
  href: string
  readAt: string | null
  createdAt: string
  seq: number | null
  alert: Alert
  class: string
  category: Category
  count: number
  updatedAt: string | null
  archivedAt: string | null
  actor: NotificationActor | null
  entity: NotificationEntity | null
  reason: string | null
}

/** The top-level keys the contract adds to `GET /api/notifications`. Optional:
 *  an api that predates it serves none of them. */
export interface NotificationsPayloadV2Fields {
  /** Unread rows of a For-you class (Needs you + Direct). */
  unreadForYou?: number
  /** The person's notify seq, read in the same statement (one snapshot) as
   *  the rows: every row with a seq at or below it had committed when this
   *  list was read (I-10). The list is still only the newest 50 by createdAt,
   *  so this is NOT a complete read through `seq`: only a `?since` read whose
   *  `more` is false advances an arrival record's `seq` (I-20). */
  seq?: number
  classes?: ClassInfo[]
  channels?: ChannelsPrefs
  dnd?: DndWire
  emailFrequency?: EmailFrequency
  features?: NotifyFeature[]
  /** The next page's cursor (W2-H); null until then. */
  next?: string | null
}

// ── I-16 / I-17 / I-19: the stream's new frames ──────────────────────────────

/** `notification`, v2: the seq the write took. */
export interface NotificationEventV2 {
  type: 'notification'
  notificationId: string
  seq?: number
}

/** `notifications` (read-sync), v2: which rows were read, so every client
 *  closes its OS banners for them. Over 100: no ids and `resync`. */
export interface NotificationsEventV2 {
  type: 'notifications'
  unread: number
  unreadForYou?: number
  readIds?: string[]
  all?: boolean
  resync?: boolean
}

/** Which of the person's clients they are on — published only when it changes. */
export interface PresenceEvent {
  type: 'presence'
  looking: string | null
  active: string | null
  clients: number
}

/** The stream's replay state (W2-H). */
export interface ResyncEvent {
  type: 'resync'
  seq: number
  complete: boolean
}

/** The session behind the stream ended; it closes after this (W2-H). */
export interface SessionEvent {
  type: 'session'
  state: 'ended'
}

// ── I-17: presence ───────────────────────────────────────────────────────────

export type ClientKind = 'web' | 'desktop'
export type ClientState = 'looking' | 'active' | 'idle' | 'hidden'

/** `PUT /api/me/presence`'s body. `clientId`: `[A-Za-z0-9_-]{8,64}`. */
export interface PresenceBody {
  clientId: string
  kind: ClientKind
  state: ClientState
  osIdleMs?: number
}

/** `GET /api/me/presence`. */
export interface PresenceSnapshot {
  looking: string | null
  active: string | null
  clients: Array<{ clientId: string; kind: ClientKind; state: ClientState; at: string }>
}

// ── The devices list and the per-device test ─────────────────────────────────

/** One device browser (`GET /api/push/devices`). `lastError` is plain text of
 *  at most 120 characters. */
export interface Device {
  id: string
  label: string
  createdAt: string
  lastSeenAt: string
  lastAttemptAt: string | null
  lastStatus: number | null
  lastError: string | null
  failures: number
  thisSession: boolean
}

/** `POST /api/notifications/test`'s `device`. */
export interface DeviceTest {
  id: string
  status: number | null
  ms: number
  error: string | null
}

// ── I-20: the arrival record, v2 ─────────────────────────────────────────────

/** `localStorage[ARRIVALS_KEY][userId]`, v2: wave 1's `hw`/`seen`/`at` plus
 *  `v` and `seq` — the notify seq this browser has read THROUGH (a complete
 *  `?since` read). A wave-1 reader ignores both; a wave-1 shell's merge keeps
 *  them. */
export interface ArrivalRecordV2 {
  hw: string
  seen: Record<string, string>
  at: number
  v?: 2
  seq?: number
}

// ── Shared names ─────────────────────────────────────────────────────────────

/** The catch-up summary's tag, on the page and in the shell's watcher (I-21). */
export const SUMMARY_TAG = 'talaria-summary'

/** The group-key conventions (I-24) — the server's `group_key::*`, spelled the
 *  same. */
export const GROUP = {
  dm: (channelId: string) => `dm:${channelId}`,
  thread: (conversationId: string) => `thread:${conversationId}`,
  taskStatus: (taskId: string) => `task:${taskId}:status`,
  taskComments: (taskId: string) => `task:${taskId}:comments`,
  channelActivity: (channelId: string) => `channel:${channelId}:activity`,
  channelThread: (rootMessageId: string) => `reply:${rootMessageId}`,
  mcpDown: (serverId: string) => `mcp:${serverId}:down`,
  approval: (approvalId: string) => `approval:${approvalId}`,
  outreach: (agent: string, day: string) => `outreach:${agent}:${day}`,
} as const

/** The entity kinds a row can be about (the server's `entity::*`). */
export const ENTITY = {
  task: 'task',
  channel: 'channel',
  conversation: 'conversation',
  approval: 'approval',
  mcpServer: 'mcp_server',
  plan: 'plan',
  researchRun: 'research_run',
  kbDoc: 'kb_doc',
} as const

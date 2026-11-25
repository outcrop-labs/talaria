// A board's ClickUp sync, as the board's settings, its header chip and its
// tickets read it. The engine is api/crates/talaria-board-sync and its contract
// is docs/BOARD-SYNC.md; this file is the client's copy of the wire, the reads
// and writes through the fetch-json verbs, and the pure derivations the three
// surfaces share (the chip's state, the mapping table, the remote-state words),
// pinned by board-sync.test.ts.
//
// WHO: any board member reads the status (the chip, a ticket's source line);
// an owner connects, edits the mapping and cadence, pauses and resumes,
// replaces the credential and disconnects; an owner or editor presses Sync now
// or re-syncs one ticket — never while the sync is paused. The server enforces
// every one of those — the UI only stops offering what it would refuse.
//
// A PAUSE FREEZES THE SYNC (talaria-board-sync ops.rs): no scheduled run, no
// Sync now, no ticket re-sync until an owner resumes it. Every surface says who
// paused it and when (`pausedLine`), and how it resumes.

import { createQuery } from '@tanstack/svelte-query'
import { delJson, getJson, postJson, putJson } from '@/lib/fetch-json'
import { resolve, type MaybeGetter } from '@/lib/reactive-arg'
import type { BoardStatus } from '@/lib/statuses'
import { OFF_BOARD_STATUSES } from '@/lib/task-const'

// ── The wire ────────────────────────────────────────────────────────────────

export type SyncState = 'active' | 'paused' | 'needs_credential' | 'disconnected'
export type RemoteState = 'active' | 'archived' | 'deleted' | 'moved' | 'hidden'
export type RunState = 'queued' | 'running' | 'awaiting' | 'done' | 'error' | 'cancelled'

/** One thing a person can act on: per ticket (an unmapped status, an unknown
 *  assignee, an oversized file) or for the whole run (a refused credential, a
 *  list that is gone, a held sweep). */
export interface SyncProblem {
  kind: string
  detail: string
  fix: string
  ticketId?: string
  ticketRef?: string
  clickupUrl?: string
}

export interface SyncWarning {
  kind: string
  detail: string
  fix: string
}

export interface SyncOptions {
  includeClosed: boolean
  comments: boolean
  attachments: boolean
  notifyApps: boolean
}

export const DEFAULT_OPTIONS: SyncOptions = { includeClosed: false, comments: true, attachments: true, notifyApps: false }

/** One of a ClickUp list's statuses (`type` is ClickUp's: open, custom, done,
 *  closed — shown, never used to map). */
export interface ListStatus {
  status: string
  color?: string | null
  type?: string | null
  orderindex?: number
}

export interface SyncRun {
  id: string
  state: RunState
  error: string | null
  result: Record<string, unknown> | null
  finishedAt: string | null
}

/** GET /api/boards/{id}/sync → `sync`. */
export interface BoardSync {
  id: string
  provider: 'clickup'
  state: SyncState
  listId: string
  listPath: string | null
  listUrl: string | null
  workspaceId: string | null
  account: { username?: string | null; email?: string | null } | null
  credentialHint: string | null
  statusMap: Record<string, string>
  mapVersion: number
  remapPending: boolean
  options: SyncOptions
  intervalMinutes: number | null
  lastSyncedAt: string | null
  nextSyncAt: string | null
  lastSweepAt: string | null
  overdue: boolean
  schedulerArmed: boolean
  liveRun: { id: string; phase: string } | null
  lastRun: SyncRun | null
  connectedBy: { id: string; name: string | null; email: string | null; onBoard: boolean } | null
  /** Who paused the sync, while it is paused; null when nobody is on record. */
  pausedBy: { id: string; name: string | null; email: string | null } | null
  pausedAt: string | null
  warnings: SyncWarning[]
  problems: SyncProblem[]
  problemCount: number
  statuses: ListStatus[] | null
  /** Ticket id → its task's state in ClickUp, for the tickets whose task is
   *  not active there (archived, deleted, moved, hidden). */
  remoteStates: Record<string, RemoteState>
  createdAt: string
}

export interface BoardSyncStatus {
  sync: BoardSync | null
  /** When nothing is connected: the list the board was last disconnected from. */
  disconnected: { listPath: string | null; disconnectedAt: string | null } | null
}

export interface ClickUpAccount {
  username?: string | null
  email?: string | null
}
export interface ClickUpNamed {
  id: string
  name: string
}
export interface ClickUpListRef extends ClickUpNamed {
  taskCount?: number | null
}

/** POST …/sync/browse: the account, plus the one level asked for. */
export interface BrowseLevel {
  account: ClickUpAccount
  workspaces?: ClickUpNamed[]
  spaces?: ClickUpNamed[]
  folders?: ClickUpNamed[]
  lists?: ClickUpListRef[]
}

export interface BrowseInput {
  token?: string
  teamId?: string
  spaceId?: string
  folderId?: string
}

/** A column the mapping may target, as the preview offers it. */
export interface SyncTarget {
  key: string
  label: string
  category: string
  /** Entering it starts work for the agents assigned to a ticket. */
  agentStart: boolean
  terminal: boolean
}

export type PersonOutcome = 'member' | 'not-on-board' | 'no-account' | 'no-email'

/** POST …/sync/preview — what connecting would do. Nothing is written. */
export interface SyncPreview {
  list: { id: string; path: string; taskCount: number | null }
  statuses: Array<{ name: string; color: string | null; type: string | null; suggested: string | null }>
  columns: SyncTarget[]
  /** The column an unmapped status lands in, or null when the board has none
   *  (then such a task is refused with a problem). */
  intake: string | null
  sample: Array<{
    id: string
    title: string
    clickupStatus: string
    column: string | null
    unmapped: boolean
    priority: string
    due: string | null
    assignees: string[]
    tags: string[]
    closed: boolean
  }>
  people: Array<{ clickupName: string; email: string | null; outcome: PersonOutcome }>
  newLabels: string[]
  closedTasks: number
  oversizeInSample: number
  estimate: { requests: number; perMinute: number; minutes: number; bytes: number }
  statsFrom: { tasks: number; partial: boolean; attachmentSample: number }
}

export interface ConnectInput {
  token: string
  listId: string
  statusMap: Record<string, string>
  options: SyncOptions
  intervalMinutes: number | null
}

export interface UpdateInput {
  statusMap?: Record<string, string>
  options?: Partial<SyncOptions>
  intervalMinutes?: number | null
  paused?: boolean
}

// ── Reads and writes ────────────────────────────────────────────────────────

export const syncKey = (boardId: string | null) => ['board-sync', boardId] as const

/** While a run is going (or a mapping edit waits for one), the projection is
 *  re-read every few seconds so the phase line moves; otherwise the board's
 *  live stream (useBoardLive) invalidates it on the sync's batch events. */
export const SYNC_POLL_MS = 3000
export const pollWhileLive = (data: BoardSyncStatus | undefined): number | false =>
  data?.sync && (data.sync.liveRun || data.sync.remapPending) ? SYNC_POLL_MS : false

export function useBoardSync(boardId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const id = resolve(boardId)
    return {
      queryKey: syncKey(id),
      enabled: !!id,
      queryFn: (): Promise<BoardSyncStatus> => getJson<BoardSyncStatus>(`/api/boards/${id}/sync`),
      refetchInterval: (q: { state: { data?: BoardSyncStatus } }) => pollWhileLive(q.state.data),
    }
  })
}

export const browseClickUp = (boardId: string, input: BrowseInput) =>
  postJson<BrowseLevel>(`/api/boards/${boardId}/sync/browse`, input)

export const previewClickUp = (boardId: string, input: { token?: string; listId: string }) =>
  postJson<SyncPreview>(`/api/boards/${boardId}/sync/preview`, input)

export const connectClickUp = (boardId: string, input: ConnectInput) =>
  postJson<{ sync: BoardSync; runId: string }>(`/api/boards/${boardId}/sync`, input)

export const updateBoardSync = (boardId: string, input: UpdateInput) =>
  putJson<{ sync: BoardSync }>(`/api/boards/${boardId}/sync`, input)

export const replaceSyncCredential = (boardId: string, token: string) =>
  putJson<{ sync: BoardSync }>(`/api/boards/${boardId}/sync/credential`, { token })

export const syncNow = (boardId: string) =>
  postJson<{ runId: string; live: boolean }>(`/api/boards/${boardId}/sync/runs`)

export const disconnectBoardSync = (boardId: string) => delJson<{ ok: true }>(`/api/boards/${boardId}/sync`)

export const resyncTicket = (taskId: string) => postJson<{ runId: string }>(`/api/tasks/${taskId}/resync`)

// ── Time words ──────────────────────────────────────────────────────────────

const span = (ms: number): string => {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  if (h < 48) return `${h}h`
  return `${Math.floor(h / 24)}d`
}

/** "12m ago" / "just now" for a past instant. */
export function ago(iso: string | null, now: number): string | null {
  if (!iso) return null
  const d = now - Date.parse(iso)
  if (Number.isNaN(d)) return null
  return d < 45_000 ? 'just now' : `${span(d)} ago`
}

/** "in 40m" for a future instant, "due now" once it has passed. */
export function until(iso: string | null, now: number): string | null {
  if (!iso) return null
  const d = Date.parse(iso) - now
  if (Number.isNaN(d)) return null
  return d <= 30_000 ? 'due now' : `in ${span(d)}`
}

// ── Cadence ─────────────────────────────────────────────────────────────────

/** The cadences the engine accepts (store::interval_allowed). */
export const CADENCES = [
  { id: 'manual', minutes: null, label: 'Manual' },
  { id: '15', minutes: 15, label: '15 min' },
  { id: '60', minutes: 60, label: 'Hourly' },
  { id: '1440', minutes: 1440, label: 'Daily' },
] as const
export type CadenceId = (typeof CADENCES)[number]['id']
export const DEFAULT_CADENCE: CadenceId = '60'

export const cadenceId = (minutes: number | null): CadenceId =>
  CADENCES.find((c) => c.minutes === minutes)?.id ?? 'manual'
export const cadenceMinutes = (id: CadenceId): number | null => CADENCES.find((c) => c.id === id)?.minutes ?? null

export function cadenceLabel(minutes: number | null): string {
  if (minutes == null) return 'Manual — only Sync now'
  if (minutes === 15) return 'Every 15 minutes'
  if (minutes === 60) return 'Hourly'
  if (minutes === 1440) return 'Daily'
  return `Every ${minutes} minutes`
}

// ── The chip ────────────────────────────────────────────────────────────────

export type ChipTone = 'ok' | 'live' | 'warn' | 'danger' | 'idle'

/** The status dot for each tone (components/ui/chip.ts DotStatus). */
export const DOT_FOR_TONE: Record<ChipTone, 'ok' | 'accent' | 'warn' | 'danger' | 'idle'> = {
  ok: 'ok',
  live: 'accent',
  warn: 'warn',
  danger: 'danger',
  idle: 'idle',
}

export interface ChipView {
  tone: ChipTone
  /** The words on the chip, after "ClickUp ·". */
  text: string
  /** One sentence for the tooltip and the popover's state line. */
  detail: string
}

const runProblem = (sync: BoardSync, kind: string) => sync.problems.find((p) => p.kind === kind && !p.ticketId)

/** "Paused by Olive Owner · 5m ago": who froze the sync and since when, as much
 *  of it as is on record (a pause from before the record says only "Paused"). */
export function pausedLine(sync: Pick<BoardSync, 'pausedBy' | 'pausedAt'>, now: number): string {
  const who = sync.pausedBy?.name || sync.pausedBy?.email || null
  const when = ago(sync.pausedAt, now)
  return `Paused${who ? ` by ${who}` : ''}${when ? ` · ${when}` : ''}`
}

/** Why Sync now and a ticket's re-sync are off while the sync is paused — the
 *  engine's 409 in the reader's terms. `canResume`: the reader is an owner. */
export function pausedHint(sync: Pick<BoardSync, 'pausedBy' | 'pausedAt'>, now: number, canResume: boolean): string {
  return `${pausedLine(sync, now)} — ${canResume ? 'resume it in Board settings → Sync to sync again' : 'a board owner resumes it in Board settings → Sync'}`
}

// ── Toasts ──────────────────────────────────────────────────────────────────
// The toast host shows a body on ONE truncated line (Toasts.svelte), about 45
// characters of IBM Plex Sans at its width and fewer in a wider fallback font,
// so every body here stays within TOAST_BODY_MAX.

export const TOAST_BODY_MAX = 42

/** After Pause: nothing reads ClickUp — no schedule, Sync now or re-sync. */
export const PAUSED_TOAST = { title: 'Sync paused', body: 'Nothing reads ClickUp until you resume it.' } as const

/** After a mapping save: the remap moves what the old mapping placed, and a
 *  ticket a person moved since stays where they put it. */
export const MAPPING_SAVED_TOAST = { title: 'Mapping saved', body: 'Tickets follow it; ones people moved stay.' } as const

const CADENCE_PHRASE: Record<number, string> = { 15: 'every 15 min', 60: 'every hour', 1440: 'once a day' }

/** The toast after Resume, from the projection the PUT answered. A scheduled
 *  sync is due at once (the engine sets next_sync_at to now), so the
 *  scheduler's next tick catches the board up — unless a run is still live
 *  (a mapping change's remap, which goes on while paused): the scheduler
 *  waits that run out, so the toast says after it, not within a minute. A
 *  manual one waits for Sync now; with the schedule off on this instance only
 *  Sync now syncs. */
export function resumedToast(sync: Pick<BoardSync, 'intervalMinutes' | 'schedulerArmed' | 'liveRun'>): {
  title: string
  body: string
} {
  const title = 'Sync resumed'
  if (sync.intervalMinutes == null) return { title, body: 'Manual — press Sync now to catch up.' }
  if (!sync.schedulerArmed) return { title, body: 'Scheduled syncs are off — press Sync now.' }
  const every = CADENCE_PHRASE[sync.intervalMinutes] ?? `every ${sync.intervalMinutes} min`
  return { title, body: `Syncs ${sync.liveRun ? 'after this run' : 'within a minute'}, then ${every}.` }
}

/** What the board's chip says, most urgent first: a dead credential, a run
 *  going, paused, a list that is gone, a failed run, the schedule switched off
 *  on this instance, overdue, then the last sync. NEVER SILENTLY STALE — every
 *  state the projection can be in has words here. `canResume` (the reader is a
 *  board owner) picks how a paused sync's sentence ends. */
export function chipView(sync: BoardSync, now: number, opts: { canResume?: boolean } = {}): ChipView {
  if (sync.state === 'needs_credential') {
    return {
      tone: 'danger',
      text: 'needs a new token',
      detail: 'ClickUp refused the sync’s token. A board owner replaces it in Board settings → Sync; the mapping is kept.',
    }
  }
  if (sync.liveRun) {
    return { tone: 'live', text: 'syncing', detail: sync.liveRun.phase || 'Starting the sync' }
  }
  const last = ago(sync.lastSyncedAt, now)
  if (sync.state === 'paused') {
    // A PAUSE FREEZES THE SYNC: nothing runs until an owner resumes it, so it
    // outranks what the last run said (that comes back on Resume, if it still
    // holds). A live run while paused is a mapping edit's remap, above.
    return {
      tone: 'idle',
      text: 'paused',
      detail: `${pausedLine(sync, now)} — ${opts.canResume ? 'resume to sync' : 'nothing syncs until a board owner resumes it'}. ${last ? `Last synced ${last}.` : 'It has not synced yet.'}`,
    }
  }
  const gone = runProblem(sync, 'list-gone')
  if (gone) return { tone: 'danger', text: 'list not found', detail: gone.detail }
  if (sync.lastRun?.state === 'error') {
    return {
      tone: 'danger',
      text: 'last sync failed',
      detail: sync.lastRun.error ? `The last sync failed: ${sync.lastRun.error}` : 'The last sync failed.',
    }
  }
  // Before `overdue`: with the schedule off on this instance nothing advances
  // next_sync_at (only the scheduler's claim does), so every scheduled
  // connection reads overdue for good — the reason is the switch, not a late run.
  if (sync.intervalMinutes != null && !sync.schedulerArmed) {
    return {
      tone: 'warn',
      text: last ? `synced ${last}` : 'not synced yet',
      detail: `${last ? `Last synced ${last}.` : 'It has not synced yet.'} Scheduled syncs are off on this instance — use Sync now.`,
    }
  }
  if (sync.overdue) {
    return {
      tone: 'warn',
      text: 'sync overdue',
      detail: `The scheduled sync was due ${ago(sync.nextSyncAt, now) ?? 'a while ago'} and has not run. Press Sync now.`,
    }
  }
  if (!last) return { tone: 'idle', text: 'not synced yet', detail: 'The first sync has not finished yet.' }
  if (sync.intervalMinutes == null) {
    return { tone: 'ok', text: `synced ${last}`, detail: `Last synced ${last}. Manual: it syncs only on Sync now.` }
  }
  const next = until(sync.nextSyncAt, now)
  return { tone: 'ok', text: `synced ${last}`, detail: `Last synced ${last}.${next ? ` Next sync ${next}.` : ''}` }
}

// ── The mapping table ───────────────────────────────────────────────────────

/** The off-board statuses: legal on every board, never a column, never a target. */
const OFF_BOARD = new Set<string>(OFF_BOARD_STATUSES)

/** The board's columns as mapping targets: every column plus Blocked; never
 *  the off-board failed/cancelled. The same set the engine's offered_targets
 *  answers in the preview, rebuilt from the board's own statuses so the
 *  connected panel can edit the table without asking ClickUp. */
export function mappingTargets(statuses: readonly BoardStatus[]): SyncTarget[] {
  return statuses
    .filter((s) => !OFF_BOARD.has(s.key))
    .map((s) => ({
      key: s.key,
      label: s.label,
      category: s.category,
      agentStart: s.agentStart,
      terminal: s.category === 'done',
    }))
}

/** The sync intake: the first open column that is not an agent pickup column
 *  (mapping.rs Columns::intake). Unmapped statuses land here. */
export function syncIntake(targets: readonly SyncTarget[]): SyncTarget | null {
  return targets.find((t) => t.category === 'open' && !t.agentStart && !t.terminal) ?? null
}

/** The sentence under a mapping row: what landing in that column means. */
export function targetNote(target: SyncTarget | null, intake: SyncTarget | null): string | null {
  if (!target) {
    return intake
      ? `Unmapped: lands in ${intake.label} and is listed as a problem.`
      : 'Unmapped, and this board has no intake column without agent pickup: the task is refused and listed as a problem.'
  }
  if (target.agentStart) {
    return `${target.label} starts agent work. The sync never adds agents, and it holds a move into this column while a ticket has agents assigned.`
  }
  if (target.terminal) {
    return `${target.label} is a done column: a task closed in ClickUp closes its ticket here, and a workchain step hands on as if a person closed it.`
  }
  if (target.category === 'review') {
    return 'The sync moves tickets into review but never out of it — the reviewer decides.'
  }
  if (target.category === 'blocked') {
    return 'The sync moves tickets into Blocked but never out of it — a person unblocks.'
  }
  return null
}

export interface MappingRow {
  /** The ClickUp status as ClickUp spells it. */
  name: string
  /** The table's key: the name, trimmed and lowercased (validate_map does the same). */
  key: string
  color: string | null
  type: string | null
  /** The saved status is no longer one of the list's statuses. */
  stale: boolean
}

export const statusKey = (name: string) => name.trim().toLowerCase()

/** The table's rows: the list's statuses in ClickUp's order, then any status
 *  the saved table still names that the list no longer has. */
export function mappingRows(statuses: readonly ListStatus[] | null, saved: Record<string, string>): MappingRow[] {
  const rows: MappingRow[] = [...(statuses ?? [])]
    .sort((a, b) => (a.orderindex ?? 0) - (b.orderindex ?? 0))
    .map((s) => ({ name: s.status, key: statusKey(s.status), color: s.color ?? null, type: s.type ?? null, stale: false }))
  const seen = new Set(rows.map((r) => r.key))
  for (const name of Object.keys(saved)) {
    const key = statusKey(name)
    if (seen.has(key)) continue
    seen.add(key)
    rows.push({ name, key, color: null, type: null, stale: true })
  }
  return rows
}

/** A draft table as the wire takes it: unmapped rows ('' or missing) left out,
 *  keys lowercased, targets that are no longer columns dropped. */
export function statusMapOf(draft: Record<string, string>, targets: readonly SyncTarget[]): Record<string, string> {
  const keys = new Set(targets.map((t) => t.key))
  const out: Record<string, string> = {}
  for (const [name, col] of Object.entries(draft)) {
    if (col && keys.has(col)) out[statusKey(name)] = col
  }
  return out
}

export function sameMap(a: Record<string, string>, b: Record<string, string>): boolean {
  const ka = Object.keys(a)
  if (ka.length !== Object.keys(b).length) return false
  return ka.every((k) => b[k] === a[k])
}

/** Where a preview's sample task lands under the table as the person has it now
 *  (the server's sample used the suggested table). */
export function landsIn(clickupStatus: string, draft: Record<string, string>, intake: string | null): { column: string | null; unmapped: boolean } {
  const col = draft[statusKey(clickupStatus)]
  return col ? { column: col, unmapped: false } : { column: intake, unmapped: true }
}

// ── People, files, runs ─────────────────────────────────────────────────────

export const PERSON_OUTCOME: Record<PersonOutcome, { label: string; tone: 'success' | 'warn' | 'neutral' }> = {
  member: { label: 'assigned', tone: 'success' },
  'not-on-board': { label: 'on Talaria, not on this board — left unassigned', tone: 'warn' },
  'no-account': { label: 'no Talaria account — left unassigned', tone: 'warn' },
  'no-email': { label: 'ClickUp hides their email — left unassigned', tone: 'neutral' },
}

/** The words after "Last sync": what the connection's latest run did.
 *  `lastRun` is whichever run started last — a one-ticket re-sync or a mapping
 *  edit's remap too — but only a scheduled or manual run walks the list and
 *  moves `lastSyncedAt` (run.rs finish), so a ticket or remap run is named for
 *  what it was rather than read as the board's last sync. */
export function lastRunNote(result: Record<string, unknown> | null | undefined): string | null {
  const mode = result && typeof result === 'object' ? result.mode : undefined
  if (mode === 'ticket') return 'latest run re-synced one ticket'
  if (mode === 'remap') {
    const counts = runSummary(result)
    return `latest run applied the mapping${counts ? ` (${counts})` : ''}`
  }
  return runSummary(result)
}

/** Where a one-ticket re-sync stands, read off a projection fetched after it
 *  was started: still going, finished (done / error / cancelled), or `ended`
 *  — its run is neither live nor the latest any more because a later run (the
 *  schedule held back behind it, or someone's Sync now) took `lastRun`, so it
 *  finished without this view seeing how. */
export type ResyncOutcome = 'going' | 'done' | 'error' | 'ended'
export function resyncOutcome(sync: BoardSync, runId: string): ResyncOutcome {
  if (sync.liveRun?.id === runId) return 'going'
  const run = sync.lastRun?.id === runId ? sync.lastRun : null
  if (!run) return 'ended'
  if (run.state === 'done') return 'done'
  if (run.state === 'error' || run.state === 'cancelled') return 'error'
  return 'going'
}

/** The finished run's counts in one line, or null when there is nothing to say. */
export function runSummary(result: Record<string, unknown> | null | undefined): string | null {
  if (!result || typeof result !== 'object') return null
  if (typeof result.stopped === 'string') return `Stopped: ${result.stopped}.`
  const n = (k: string) => (typeof result[k] === 'number' ? (result[k] as number) : 0)
  const parts: string[] = []
  if (n('created')) parts.push(`${n('created')} new`)
  if (n('updated')) parts.push(`${n('updated')} updated`)
  if (n('unchanged')) parts.push(`${n('unchanged')} unchanged`)
  if (n('comments')) parts.push(`${n('comments')} comment${n('comments') === 1 ? '' : 's'}`)
  if (n('attachments')) parts.push(`${n('attachments')} file${n('attachments') === 1 ? '' : 's'}`)
  if (n('marked')) parts.push(`${n('marked')} marked`)
  if (n('held')) parts.push(`${n('held')} held`)
  if (!parts.length) return n('checked') ? `${n('checked')} checked, nothing changed` : null
  return parts.join(' · ')
}

// ── A synced ticket ─────────────────────────────────────────────────────────

/** Who the sync is on a ticket: its `createdBy` and the actor of its activity
 *  lines (talaria-board-sync SYNC_ACTOR). */
export const SYNC_ACTOR = 'clickup-sync'

/** A ticket's actor as a person reads it: the sync's id is "ClickUp sync". */
export function actorLabel<T extends string | null | undefined>(actor: T): T | 'ClickUp sync' {
  return actor === SYNC_ACTOR ? 'ClickUp sync' : actor
}

/** The ClickUp task id behind a ticket's `externalKey` ('clickup:<id>'), or null. */
export function clickupTaskId(externalKey: string | null | undefined): string | null {
  const m = /^clickup:(.+)$/.exec(externalKey ?? '')
  return m ? m[1]! : null
}

/** ClickUp's own short link to a task. */
export const clickupTaskUrl = (id: string) => `https://app.clickup.com/t/${encodeURIComponent(id)}`

export const REMOTE_STATE_LABEL: Record<Exclude<RemoteState, 'active'>, string> = {
  archived: 'Archived in ClickUp',
  deleted: 'Deleted in ClickUp',
  moved: 'Moved to another ClickUp list',
  hidden: 'No longer visible to the sync',
}

/** The chip's words for a ticket whose task is not active in ClickUp. */
export function remoteStateLabel(state: string | null | undefined): string | null {
  return state && Object.hasOwn(REMOTE_STATE_LABEL, state) ? REMOTE_STATE_LABEL[state as keyof typeof REMOTE_STATE_LABEL] : null
}

/** The account a token acts as, in one phrase. */
export function accountLabel(a: ClickUpAccount | null | undefined): string {
  if (!a) return 'an unknown ClickUp member'
  if (a.username && a.email) return `${a.username} (${a.email})`
  return a.username || a.email || 'an unknown ClickUp member'
}

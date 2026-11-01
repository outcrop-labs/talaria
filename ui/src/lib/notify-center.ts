// The notification center: the bell's dropdown and the /notifications page.
// What a person sees, Slack/Linear/GitHub style — For you and All, rows grouped
// by the thing they are about with a count, Done, read/unread, filters, keyset
// paging — built on the contract's row wire (I-11), the class registry (I-12)
// and the inbox API (W2-H: views, filters, `next`, archive/unarchive/unread).
//
// THE DEGRADE RULE (I-23). Everything the inbox API adds is gated on
// `features`: without `inbox-v2` there is no Done, no paging and no server
// filter, and For you / All filter the newest 50 the bell already holds —
// wave 1's list, sliced. A row without the v2 fields (an api older than the
// contract) is For you: the badge counts it, as wave 1's did.
//
// The pure half (views, grouping, labels, filters) is tested in
// notify-center.test.ts; the hooks at the bottom are the only part that
// touches the network, through the one fetch door (lib/fetch-json).

import { createInfiniteQuery, createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJsonGone, errorMessage, getJson, getList, HttpError, patchJson, postJsonOr, putJson } from '@/lib/fetch-json'
import { toastError } from '@/lib/toast.svelte'
import type { Notification } from './notify-classes'
import type { NotificationsPayload } from './notifications'
import type { DirectoryUser } from './users'
import { useProfilePrefs } from './muse.svelte'
import { presetZone } from './notify-dnd'
import { readToggle } from './notify-keys'
import { detectedZone } from './timezone'
import type {
  Category,
  ChannelsPatch,
  ChannelsPrefs,
  ClassInfo,
  Device,
  DeviceTest,
  DndSchedule,
  DndWire,
  EmailFrequency,
  NotificationRowV2,
  PresenceSnapshot,
} from './notify-wire'

/** A row as the bell holds it: wave 1's seven keys, the v2 ones when served. */
export type CenterRow = Notification & Partial<Omit<NotificationRowV2, keyof Notification>>

// ── Features ─────────────────────────────────────────────────────────────────

/** The flags the api serves (I-23). The center reads three of W2-H's. */
export const INBOX_V2 = 'inbox-v2'
export const ARCHIVE = 'archive'

export function featureSet(p: { features?: readonly string[] } | null | undefined): ReadonlySet<string> {
  return new Set(p?.features ?? [])
}

/** Done/archive, mark unread, paging and the server's filters. */
export const hasInboxV2 = (f: ReadonlySet<string>): boolean => f.has(INBOX_V2)

// ── Views and the badge ──────────────────────────────────────────────────────

export type CenterView = 'foryou' | 'all' | 'done'

/** For you = Needs you + Direct (I-12, O-10). */
export const FOR_YOU: readonly Category[] = ['needs_you', 'direct']

export const isForYou = (row: Pick<CenterRow, 'category'>): boolean => !row.category || FOR_YOU.includes(row.category)

/** The bell's badge (O-10): unread For you, else (an api older than the
 *  contract) every unread row, as wave 1 counted. */
export function badgeCount(p: { unread?: number; unreadForYou?: number } | null | undefined): number {
  return p?.unreadForYou ?? p?.unread ?? 0
}

/** Whether a row belongs in a view — the client-side half, for an api with
 *  no inbox API (the newest 50, sliced) and for hiding a row the moment it is
 *  marked Done. */
export function inView(row: CenterRow, view: CenterView): boolean {
  if (view === 'done') return !!row.archivedAt
  if (row.archivedAt) return false
  return view === 'all' || isForYou(row)
}

export const VIEW_LABEL: Record<CenterView, string> = { foryou: 'For you', all: 'All', done: 'Done' }

// ── Labels ───────────────────────────────────────────────────────────────────

/** Why this row reached you ("Mentioned you"): the server's `reason`, else
 *  the registry's reason for its kind, else nothing. */
export function reasonOf(row: CenterRow, classes: readonly ClassInfo[] | undefined): string | null {
  if (row.reason) return row.reason
  for (const c of classes ?? []) {
    const k = c.kinds.find((x) => x.id === row.kind)
    if (k) return k.reason
  }
  return null
}

const COUNT_NOUN: Record<string, (n: number) => string> = {
  dm: (n) => `${n} messages`,
  'agent-reply': (n) => `${n} replies`,
  'agent-outreach': (n) => `${n} messages`,
  'channel-message': (n) => `${n} messages`,
  'thread-reply': (n) => `${n} replies`,
  'task-comment': (n) => `${n} comments`,
  'task-status': (n) => `Moved ${n} times`,
  mcp_server_down: (n) => `Down ${n} times`,
  approval_pending: (n) => `Asked ${n} times`,
}

/** "3 messages", "Moved 4 times" — a coalesced row's count (W2-P), or null
 *  for a row that stands for one event. */
export function countLabel(row: Pick<CenterRow, 'kind' | 'count'>): string | null {
  const n = row.count ?? 1
  if (!Number.isFinite(n) || n <= 1) return null
  return (COUNT_NOUN[row.kind] ?? ((k: number) => `${k} updates`))(n)
}

/** What an entity type is called in a group header. */
export const ENTITY_LABEL: Record<string, string> = {
  task: 'Ticket',
  channel: 'Channel',
  conversation: 'Conversation',
  approval: 'Approval',
  mcp_server: 'MCP server',
  plan: 'Plan',
  research_run: 'Research',
  kb_doc: 'Doc',
}

export function entityKey(row: Pick<CenterRow, 'entity'>): string | null {
  const e = row.entity
  return e?.type && e.id ? `${e.type}:${e.id}` : null
}

export interface RowGroup {
  /** Stable across refetches: the entity, else the row id. */
  key: string
  entity: { type: string; id: string } | null
  rows: CenterRow[]
  /** "TAL-112 · 3 updates"; null for a single row. */
  label: string | null
}

const TICKET_KEY = /\b[A-Z][A-Z0-9]{1,9}-\d+\b/

/** A name the rows of one group share: a ticket key every one of them
 *  carries ("TAL-112"), else the entity type's word ("Ticket"). */
function groupName(rows: readonly CenterRow[], type: string): string {
  const keys = rows.map((r) => TICKET_KEY.exec(`${r.title} ${r.body}`)?.[0] ?? null)
  const first = keys[0]
  if (first && keys.every((k) => k === first)) return first
  return ENTITY_LABEL[type] ?? 'Item'
}

/** CONSECUTIVE rows about the same thing go under one header (a run of
 *  updates to one ticket reads as one item); a row about nothing, or about a
 *  thing its neighbours are not, is its own group of one. Order is kept: a
 *  group never pulls a row up past newer rows about something else. */
export function groupRows(rows: readonly CenterRow[]): RowGroup[] {
  const out: RowGroup[] = []
  for (const row of rows) {
    const k = entityKey(row)
    const last = out.at(-1)
    if (k && last && last.entity && `${last.entity.type}:${last.entity.id}` === k) {
      last.rows.push(row)
      continue
    }
    out.push({ key: k ? `${k}@${row.id}` : row.id, entity: k ? { type: row.entity!.type, id: row.entity!.id! } : null, rows: [row], label: null })
  }
  for (const g of out) {
    if (g.entity && g.rows.length > 1) g.label = `${groupName(g.rows, g.entity.type)} · ${g.rows.length} updates`
  }
  return out
}

// ── Filters ──────────────────────────────────────────────────────────────────

export interface CenterFilters {
  unread: boolean
  categories: Category[]
  /** One kind, or null for every kind. */
  kind: string | null
}

export const NO_FILTERS: CenterFilters = { unread: false, categories: [], kind: null }

export const filtersActive = (f: CenterFilters): boolean => f.unread || f.categories.length > 0 || f.kind !== null

const CATEGORY_IDS: readonly Category[] = ['needs_you', 'direct', 'following', 'workspace']

/** The filters a URL names (`?unread=1&category=direct,following&kind=dm`):
 *  the URL is the selection. Unknown categories are dropped. */
export function filtersFrom(get: (k: string) => string | null | undefined): CenterFilters {
  const cats = (get('category') ?? '')
    .split(',')
    .map((s) => s.trim())
    .filter((s): s is Category => (CATEGORY_IDS as readonly string[]).includes(s))
  const kind = (get('kind') ?? '').trim()
  return { unread: get('unread') === '1', categories: [...new Set(cats)], kind: /^[a-z0-9_-]{1,64}$/.test(kind) ? kind : null }
}

export function viewFrom(raw: string | null | undefined): CenterView {
  return raw === 'all' || raw === 'done' ? raw : 'foryou'
}

/** The inbox API's query string (W2-H). */
export function centerQuery(view: CenterView, f: CenterFilters, before?: string | null, limit = 50): string {
  const q = new URLSearchParams()
  q.set('view', view)
  if (f.unread) q.set('unread', '1')
  if (f.categories.length) q.set('category', f.categories.join(','))
  if (f.kind) q.set('kind', f.kind)
  q.set('limit', String(limit))
  if (before) q.set('before', before)
  return q.toString()
}

/** The client-side twin of the server's filters, for an api with no inbox
 *  API. A row with no category is For you's (see `isForYou`). */
export function matchesFilters(row: CenterRow, f: CenterFilters): boolean {
  if (f.unread && row.readAt) return false
  if (f.categories.length && !(row.category ? f.categories.includes(row.category) : f.categories.some((c) => FOR_YOU.includes(c)))) return false
  if (f.kind && row.kind !== f.kind) return false
  return true
}

/** The kind select's options, from the registry: each kind under its reason,
 *  limited to the chosen categories. */
export function kindOptions(classes: readonly ClassInfo[] | undefined, categories: readonly Category[] = []): Array<{ id: string; label: string }> {
  const out: Array<{ id: string; label: string }> = []
  const seen = new Set<string>()
  for (const c of classes ?? []) {
    if (categories.length && !categories.includes(c.category)) continue
    for (const k of c.kinds) {
      if (seen.has(k.id)) continue
      seen.add(k.id)
      out.push({ id: k.id, label: k.reason })
    }
  }
  return out
}

/** Pages flattened, each row once (a row that moved between pages while
 *  paging — a coalesce re-alerted it — keeps its first place). */
export function flattenPages(pages: ReadonlyArray<{ notifications: CenterRow[] }>): CenterRow[] {
  const seen = new Set<string>()
  const out: CenterRow[] = []
  for (const p of pages) {
    for (const r of p.notifications) {
      if (seen.has(r.id)) continue
      seen.add(r.id)
      out.push(r)
    }
  }
  return out
}

/** What read ↔ unread does to some rows: read the unread ones when any is
 *  unread, else make them all unread again (notify-keys `readToggle`). */
export function readToggleIds(rows: ReadonlyArray<Pick<CenterRow, 'id' | 'readAt'>>): { read: string[]; unread: string[] } {
  return readToggle(rows) === 'read'
    ? { read: rows.filter((r) => !r.readAt).map((r) => r.id), unread: [] }
    : { read: [], unread: rows.map((r) => r.id) }
}

// ── The network ──────────────────────────────────────────────────────────────

/** One page of the inbox API. */
export interface CenterPage {
  notifications: CenterRow[]
  next?: string | null
  unread: number
  unreadForYou?: number
  classes?: ClassInfo[]
  features?: string[]
}

/** A view's rows, paged by keyset (`next`). Under `['notifications', …]`, so
 *  the live stream's invalidation of the bell refreshes it too. */
export function useCenterList(args: () => { view: CenterView; filters: CenterFilters; enabled: boolean; limit?: number }) {
  return createInfiniteQuery(() => {
    const a = args()
    return {
      queryKey: ['notifications', 'center', a.view, a.filters, a.limit ?? 50],
      queryFn: ({ pageParam }: { pageParam: string }) =>
        getJson<CenterPage>(`/api/notifications?${centerQuery(a.view, a.filters, pageParam || null, a.limit ?? 50)}`),
      initialPageParam: '' as string,
      getNextPageParam: (page: CenterPage) => page.next ?? undefined,
      enabled: a.enabled,
      staleTime: 5_000,
    }
  })
}

/** The bell's payload, read from the cache the bell's own poll keeps warm —
 *  no second poll. For surfaces that need the settings (the account menu's
 *  pause, the status panel), not the rows. */
export function useNotifySettings() {
  return createQuery(() => ({
    queryKey: ['notifications'],
    queryFn: (): Promise<NotificationsPayload> => getJson<NotificationsPayload>('/api/notifications'),
    staleTime: 30_000,
  }))
}

/** Actor photos, by user id, from the people directory — read from the
 *  cache when another surface loaded it, never fetched for the bell (the
 *  directory polls; a row without a photo shows initials). */
export function useCachedPictures() {
  const q = createQuery(() => ({
    queryKey: ['users'],
    queryFn: (): Promise<DirectoryUser[]> => getList<DirectoryUser>('/api/users', 'users'),
    enabled: false,
  }))
  return {
    get current(): Record<string, string | null> {
      return Object.fromEntries((q.data ?? []).map((u) => [u.id, u.picture ?? null]))
    },
  }
}

/** The zone a pause preset is computed in (O-19): the profile's time zone,
 *  else the browser's (presetZone). */
export function useNotifyZone(dnd: () => DndWire | undefined) {
  const prefs = useProfilePrefs()
  return {
    get current(): string {
      return presetZone({ profile: prefs.data ? prefs.data.timezone : undefined, served: dnd()?.timeZone, browser: detectedZone() })
    },
  }
}

type InboxWrite =
  | { ids?: string[]; archive: true }
  | { ids: string[]; unarchive: true }
  | { ids: string[]; unread: true }
  | { ids?: string[] }

/** The inbox writes, each followed by the refetch every reader of these rows
 *  needs (the bell, its views, Home, the rail badges). A failure is a toast:
 *  the row stays where it was, which is the truth. */
export function useInboxActions() {
  const qc = useQueryClient()
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    await qc.invalidateQueries({ queryKey: ['home'] })
    await qc.invalidateQueries({ queryKey: ['unreads'] })
  }
  const write = async (body: InboxWrite, failure: string): Promise<boolean> => {
    try {
      await putJson<{ ok: true }>('/api/notifications', body)
      return true
    } catch (e) {
      toastError(failure, e)
      return false
    } finally {
      await refresh()
    }
  }
  return {
    refresh,
    /** Done: archived and read (O-11). */
    done: (ids: string[]) => (ids.length ? write({ ids, archive: true }, 'Could not mark done') : Promise.resolve(false)),
    /** Move to inbox: back in the views, still read. */
    restore: (ids: string[]) => (ids.length ? write({ ids, unarchive: true }, 'Could not move back to the inbox') : Promise.resolve(false)),
    read: (ids: string[]) => (ids.length ? write({ ids }, 'Mark as read failed') : Promise.resolve(false)),
    unread: (ids: string[]) => (ids.length ? write({ ids, unread: true }, 'Mark as unread failed') : Promise.resolve(false)),
    allRead: () => write({}, 'Mark as read failed'),
  }
}

/** What a settings PATCH answers: the settings a reload would GET. */
export interface NotifyPatchResult {
  channels?: ChannelsPrefs
  dnd?: DndWire
  emailFrequency?: EmailFrequency
  prefs?: Record<string, string>
  digest?: string
}

export interface NotifyPatch {
  channels?: ChannelsPatch
  dnd?: { pausedUntil?: string | null; schedule?: DndSchedule | null; allowNeedsYou?: boolean }
  emailFrequency?: EmailFrequency
}

/** Save the matrix, DND or the mail frequency. The answer is merged into the
 *  bell's cached payload at once (the bell's pause banner moves with the
 *  click), then the payload is refetched. Returns an error string rather than
 *  throwing: the control rolls back and says why. */
export function useNotifyPatch() {
  const qc = useQueryClient()
  return async (patch: NotifyPatch): Promise<NotifyPatchResult | { error: string }> => {
    try {
      const r = await patchJson<NotifyPatchResult>('/api/notifications', patch)
      qc.setQueryData<NotificationsPayload>(['notifications'], (old) =>
        old ? ({ ...old, ...r } as NotificationsPayload) : old,
      )
      void qc.invalidateQueries({ queryKey: ['notifications'] })
      return r
    } catch (e) {
      return { error: errorMessage(e) }
    }
  }
}

// ── Devices and the per-device test ──────────────────────────────────────────

export function useDevices(enabled: () => boolean = () => true) {
  return createQuery(() => ({
    queryKey: ['push-devices'],
    queryFn: () => getJson<{ devices: Device[] }>('/api/push/devices'),
    enabled: enabled(),
    staleTime: 10_000,
  }))
}

export const removeDevice = (id: string): Promise<void> => delJsonGone(`/api/push/devices/${encodeURIComponent(id)}`)

export type TestOutcome =
  | { kind: 'sent'; device: DeviceTest }
  | { kind: 'limited'; retryAfter: number }
  | { kind: 'unavailable' }
  | { kind: 'gone' }
  | { kind: 'error'; message: string }

/** One test push to one of the person's devices (5 per 10 minutes, O-20). */
export async function testDevice(id: string): Promise<TestOutcome> {
  try {
    const r = await postJsonOr<{ ok?: true; device?: DeviceTest; retryAfter?: number; error?: string }>(
      '/api/notifications/test',
      { deviceId: id },
      [429],
    )
    if (typeof r.retryAfter === 'number') return { kind: 'limited', retryAfter: r.retryAfter }
    if (r.device) return { kind: 'sent', device: r.device }
    return { kind: 'error', message: r.error ?? 'The server sent no result.' }
  } catch (e) {
    if (e instanceof HttpError && e.status === 501) return { kind: 'unavailable' }
    if (e instanceof HttpError && e.status === 404) return { kind: 'gone' }
    return { kind: 'error', message: errorMessage(e) }
  }
}

/** The result line under a device's Send test. */
export function testLine(t: TestOutcome): { text: string; tone: 'ok' | 'warn' | 'bad' } {
  switch (t.kind) {
    case 'sent': {
      const d = t.device
      if (d.status !== null && d.status >= 200 && d.status < 300 && !d.error) {
        return { text: `Delivered to the push service in ${d.ms} ms. A banner should appear on that device.`, tone: 'ok' }
      }
      const status = d.status === null ? 'no answer' : `HTTP ${d.status}`
      return { text: `Not delivered (${status}${d.error ? `: ${d.error}` : ''}).`, tone: 'bad' }
    }
    case 'limited':
      return { text: `Too many tests. Try again in ${retryLabel(t.retryAfter)}.`, tone: 'warn' }
    case 'unavailable':
      return { text: 'Server test not available on this instance yet.', tone: 'warn' }
    case 'gone':
      return { text: 'That device is no longer registered.', tone: 'warn' }
    case 'error':
      return { text: t.message, tone: 'bad' }
  }
}

/** "4 min", "1 min", "30 s". */
export function retryLabel(seconds: number): string {
  const s = Math.max(1, Math.ceil(seconds))
  return s >= 60 ? `${Math.ceil(s / 60)} min` : `${s} s`
}

/** The person's devices as the panel labels them: the one subscribed from
 *  this session is "This browser" — the newest such, when a browser
 *  re-subscribed in one session (the list is newest first). */
export function thisBrowserId(devices: readonly Device[]): string | null {
  return devices.find((d) => d.thisSession)?.id ?? null
}

/** The last delivery attempt in words. */
export function lastDelivery(d: Device): { text: string; ok: boolean | null } {
  if (!d.lastAttemptAt || d.lastStatus === null) return { text: 'No push sent yet', ok: null }
  const ok = d.lastStatus >= 200 && d.lastStatus < 300
  return { text: ok ? `Last delivered` : `Last push failed (HTTP ${d.lastStatus})`, ok }
}

// ── Presence (I-17), for the panel's "alerts go to" ──────────────────────────

export function usePresenceSnapshot(enabled: () => boolean) {
  return createQuery(() => ({
    queryKey: ['me-presence'],
    queryFn: () => getJson<PresenceSnapshot>('/api/me/presence'),
    enabled: enabled(),
    refetchInterval: 10_000,
  }))
}

export type AlertTarget = 'this-browser' | 'desktop' | 'other-browser' | 'none'

/** Where alerts go now, by the winner rule (I-17): the client the person is
 *  looking at, else the active one. `self` is this tab's client id when the
 *  web runtime reports it (notify-status); without it a web winner is "a
 *  browser", not claimed as this one. */
export function alertTarget(p: PresenceSnapshot | null | undefined, self: string | null): { target: AlertTarget; since?: string } {
  const winner = p?.looking ?? p?.active ?? null
  if (!winner) return { target: 'none' }
  const c = p?.clients.find((x) => x.clientId === winner)
  if (c?.kind === 'desktop') return { target: 'desktop', since: c.at }
  if (self && winner === self) return { target: 'this-browser', since: c?.at }
  return { target: 'other-browser', since: c?.at }
}

export const ALERT_TARGET_LABEL: Record<AlertTarget, string> = {
  'this-browser': 'this browser',
  desktop: 'Talaria Desktop',
  'other-browser': 'another browser or tab',
  none: 'every registered browser, as push (no device is active)',
}

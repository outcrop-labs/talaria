// What one person hears about one board, as they set it from the board (the
// bell beside Board settings, and Board settings → Notifications) and from
// Settings → Notifications → Boards (their default for every board, and each
// board's state at a glance).
//
// The server holds it and the ticket producers obey it, decided once at
// fan-out (docs/notifications/producers.md "Each board"):
//   GET/PUT/DELETE /api/boards/{id}/notify   the caller's own row for a board
//   GET /api/notifications/boards            their default + every board's state
//   PATCH /api/notifications {boards}        their default (null clears it)
// A setting is a preset (All activity, Participating, Mentions & assignments,
// Off) or Custom events over a scope, a cadence, and — feature
// `board-channels` — the board's own Alerts and Email, each on, off, or null
// (Settings → Notifications decides). The effective setting is the board's
// row, else the person's default, else the built-in (Participating): exactly
// what everyone heard before boards had a setting.
//
// Every word the forms show for an event, a preset or a cadence comes from the
// server's registry — never a client-side list. What is here is the glue: the
// control's name and glyph, the summary line, and the whole-setting write a
// change becomes (a PUT replaces the setting, so a client sends back what it
// is not changing).
//
// Framework-free apart from the query hooks at the end, so the rules are
// pinned by board-notify.test.ts.

import { createQuery } from '@tanstack/svelte-query'
import { HttpError, delJson, errorMessage, getJson, patchJson, putJson } from './fetch-json'

export type BoardPreset = 'all' | 'participating' | 'mentions' | 'off' | 'custom'
export type BoardScope = 'mine' | 'board'
export type BoardCadence = 'instant' | 'digest'
export type BoardSource = 'board' | 'default' | 'builtin'

/** A board's own channels: null where Settings → Notifications decides.
 *  `alerts` is the banner and the push together; the inbox keeps every row
 *  either way, so in-app is not a choice. */
export interface BoardChannels {
  alerts: boolean | null
  email: boolean | null
}

/** The setting that applies, always expanded. */
export interface BoardEffective {
  preset: BoardPreset
  events: string[]
  scope: BoardScope
  cadence: BoardCadence
  channels: BoardChannels
}

/** A stored setting: what the person chose, unexpanded. */
export interface BoardStored {
  preset: BoardPreset
  events?: string[]
  scope?: BoardScope
  cadence?: BoardCadence
  channels?: Partial<Record<keyof BoardChannels, boolean>>
}

export interface BoardNotifyState {
  effective: BoardEffective
  source: BoardSource
  stored: BoardStored | null
}

export interface BoardRegistry {
  events: Array<{ id: string; label: string; available: boolean; boardWideOnly: boolean }>
  presets: Array<{ id: BoardPreset; label: string; events: string[] | null; scope: BoardScope | null }>
  cadences: Array<{ id: BoardCadence; label: string; available: boolean }>
}

/** What every verb of `/api/boards/{id}/notify` answers. */
export interface BoardNotifyAnswer extends BoardNotifyState {
  default: BoardNotifyState
  registry: BoardRegistry
}

/** One board in Settings → Notifications → Boards. */
export interface NotifyBoardRow {
  id: string
  name: string
  role: string | null
  effective: BoardEffective
  source: BoardSource
}

/** `GET /api/notifications/boards`. `registry` is absent from an api older
 *  than the Settings list. */
export interface NotifyBoardsList {
  default: BoardNotifyState
  boards: NotifyBoardRow[]
  registry?: BoardRegistry
}

/** A whole setting, as written: the PUT and the default's PATCH take it. */
export interface BoardNotifyWrite {
  preset: BoardPreset
  events?: string[]
  scope?: BoardScope
  cadence?: BoardCadence
  channels?: Partial<Record<keyof BoardChannels, boolean>>
}

/** The features this reads in `GET /api/notifications` (I-23). */
export const BOARD_PREFS = 'board-prefs'
export const BOARD_CHANNELS = 'board-channels'

/** The presets the menus offer as one click, in order. Custom is a form. */
export const QUICK_PRESETS: readonly BoardPreset[] = ['all', 'participating', 'mentions', 'off'] as const

const FALLBACK_LABEL: Record<BoardPreset, string> = {
  all: 'All activity',
  participating: 'Participating',
  mentions: 'Mentions & assignments',
  off: 'Off',
  custom: 'Custom',
}

/** The registry's word for a preset (a fallback only if it names none). */
export function presetLabel(registry: BoardRegistry | null | undefined, id: BoardPreset): string {
  return registry?.presets.find((p) => p.id === id)?.label ?? FALLBACK_LABEL[id]
}

/** The registry's word for an event. */
export function eventLabel(registry: BoardRegistry | null | undefined, id: string): string {
  return registry?.events.find((e) => e.id === id)?.label ?? id
}

/** The cadences a person may choose now (a cadence with no producer yet is
 *  listed by the server and refused, so it is not offered). */
export function availableCadences(registry: BoardRegistry | null | undefined): BoardRegistry['cadences'] {
  return (registry?.cadences ?? []).filter((c) => c.available)
}

/** The events Custom offers now, in registry order. */
export function availableEvents(registry: BoardRegistry | null | undefined): BoardRegistry['events'] {
  return (registry?.events ?? []).filter((e) => e.available)
}

const lower = (s: string) => (s ? s.charAt(0).toLowerCase() + s.slice(1) : s)

/** Which tickets a setting covers, in words. */
export function scopeWords(scope: BoardScope): string {
  return scope === 'board' ? 'every ticket on the board' : 'tickets you’re in'
}

/** Where the setting that applies comes from, in words. */
export function sourceWords(source: BoardSource): string {
  return source === 'board' ? 'Set for this board' : source === 'default' ? 'Your default for every board' : 'The standard default'
}

/** The glyph the bell shows: the value, not a gear (UI-CONVENTIONS "Settings
 *  cog": a value-faced control keeps its value). */
export type BoardGlyph = 'bell-ring' | 'bell' | 'at-sign' | 'bell-off' | 'bell-dot'
export function glyphOf(e: Pick<BoardEffective, 'preset'>): BoardGlyph {
  switch (e.preset) {
    case 'all':
      return 'bell-ring'
    case 'mentions':
      return 'at-sign'
    case 'off':
      return 'bell-off'
    case 'custom':
      return 'bell-dot'
    default:
      return 'bell'
  }
}

/** The value in a few words: "participating", "all activity, daily digest". */
export function levelWords(e: Pick<BoardEffective, 'preset' | 'cadence'>, registry?: BoardRegistry | null): string {
  const level = lower(presetLabel(registry, e.preset))
  return e.preset !== 'off' && e.cadence === 'digest' ? `${level}, ${lower(cadenceLabel(registry, 'digest'))}` : level
}

function cadenceLabel(registry: BoardRegistry | null | undefined, id: BoardCadence): string {
  return registry?.cadences.find((c) => c.id === id)?.label ?? (id === 'digest' ? 'Daily digest' : 'Instant')
}

/** The bell's accessible name, and its tooltip word for word: "Board
 *  notifications: <value>", with "(your default)" when the board follows the
 *  person's default rather than a setting of its own. */
export function controlName(s: Pick<BoardNotifyState, 'effective' | 'source'> | null, registry?: BoardRegistry | null): string {
  if (!s) return 'Board notifications'
  const inherited = s.source === 'board' ? '' : ' (your default)'
  return `Board notifications: ${levelWords(s.effective, registry)}${inherited}`
}

/** A board channel's three states, as the form's switch names them. */
export type ChannelChoice = 'settings' | 'on' | 'off'
export const channelChoice = (v: boolean | null | undefined): ChannelChoice => (v === true ? 'on' : v === false ? 'off' : 'settings')
export const channelValue = (c: ChannelChoice): boolean | null => (c === 'on' ? true : c === 'off' ? false : null)

/** One sentence: what reaches the person from this board, and how.
 *  "Assigned to me, Status change and Mention — on tickets you’re in, as it
 *  happens." Off says what still comes through. Only events that can arrive
 *  today are named. */
export function summary(
  e: BoardEffective,
  registry: BoardRegistry | null | undefined,
  channelsServed: boolean,
  subject: 'board' | 'default' = 'board',
): string {
  const these = subject === 'board' ? 'this board' : 'the boards that follow it'
  if (e.preset === 'off' || e.events.length === 0) {
    return `Nothing from ${these} — assignments and @mentions included. Approvals, and tickets you watch yourself, still reach you.`
  }
  // Only what can arrive today: All covers Due too, which has no producer
  // until the registry marks it available.
  const offered = registry ? new Set(availableEvents(registry).map((ev) => ev.id)) : null
  const names = e.events.filter((id) => !offered || offered.has(id)).map((id) => eventLabel(registry, id))
  const list = names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}` : names[0]
  const when = e.cadence === 'digest' ? 'rolled up once a day' : 'as it happens'
  let line = `${list} — on ${scopeWords(e.scope)}, ${when}.`
  if (channelsServed) {
    const words = (name: string, v: boolean | null) => (v === null ? null : `${name} ${v ? 'on' : 'off'}`)
    const set = [words('Alerts', e.channels.alerts), words('Email', e.channels.email)].filter(Boolean)
    line += set.length ? ` ${set.join(', ')} for ${these}.` : ' Alerts and email follow Settings → Notifications.'
  }
  return line
}

/** The channels a write carries: only the chosen ones (null is the matrix). */
function chosenChannels(c: BoardChannels): BoardNotifyWrite['channels'] | undefined {
  const out: Partial<Record<keyof BoardChannels, boolean>> = {}
  if (c.alerts !== null) out.alerts = c.alerts
  if (c.email !== null) out.email = c.email
  return Object.keys(out).length ? out : undefined
}

/** The events Custom starts from when the setting it leaves covers none
 *  (Off): the built-in's, from the registry. */
function startingEvents(registry: BoardRegistry | null | undefined): string[] {
  return registry?.presets.find((p) => p.id === 'participating')?.events ?? ['assigned', 'status', 'comment', 'mention', 'completed']
}

/** A change, as the whole setting a PUT (or the default's PATCH) takes: the
 *  change on top of what applies now, so nothing the person did not touch
 *  moves — the cadence and the board's channels ride along, and a board
 *  that follows a default keeps that default's choices as its own.
 *  Custom keeps the events it covers now (the built-in's, leaving Off), in
 *  registry order, and drops an event that needs the whole board when the
 *  scope is the tickets you're in. */
export function writeFrom(
  current: BoardEffective,
  change: Partial<Omit<BoardNotifyWrite, 'channels'>> & { channels?: Partial<BoardChannels> },
  registry?: BoardRegistry | null,
): BoardNotifyWrite {
  const preset = change.preset ?? current.preset
  const cadence = change.cadence ?? current.cadence
  const channels = chosenChannels({ ...current.channels, ...(change.channels ?? {}) })
  const out: BoardNotifyWrite = { preset, cadence }
  if (preset === 'custom') {
    const scope = change.scope ?? current.scope
    let events = change.events ?? (current.events.length ? current.events : startingEvents(registry))
    const boardWideOnly = new Set((registry?.events ?? []).filter((e) => e.boardWideOnly).map((e) => e.id))
    if (!registry) boardWideOnly.add('created')
    if (scope === 'mine') events = events.filter((id) => !boardWideOnly.has(id))
    const order = (registry?.events ?? []).map((e) => e.id)
    if (order.length) events = [...events].sort((a, b) => order.indexOf(a) - order.indexOf(b))
    out.events = [...new Set(events)]
    out.scope = scope
  }
  if (channels) out.channels = channels
  return out
}

/** A write, expanded the way the server will answer it — what a form shows
 *  while the write is on its way. */
export function expandWrite(w: BoardNotifyWrite, registry?: BoardRegistry | null): BoardEffective {
  const named = registry?.presets.find((p) => p.id === w.preset)
  const custom = w.preset === 'custom'
  return {
    preset: w.preset,
    events: custom ? (w.events ?? []) : (named?.events ?? []),
    scope: custom ? (w.scope ?? 'mine') : (named?.scope ?? 'mine'),
    cadence: w.cadence ?? 'instant',
    channels: { alerts: w.channels?.alerts ?? null, email: w.channels?.email ?? null },
  }
}

/** The sentence a refused write shows. A 403 means the board is no longer
 *  the person's to read; anything else carries the server's own words. */
export function refusalMessage(e: unknown): string {
  if (e instanceof HttpError && e.status === 403) {
    return 'You’re no longer on this board, so its notifications can’t be changed.'
  }
  return errorMessage(e)
}

// ── The wire ─────────────────────────────────────────────────────────────────

const boardPath = (boardId: string) => `/api/boards/${encodeURIComponent(boardId)}/notify`

export const getBoardNotify = (boardId: string) => getJson<BoardNotifyAnswer>(boardPath(boardId))
export const putBoardNotify = (boardId: string, w: BoardNotifyWrite) => putJson<BoardNotifyAnswer>(boardPath(boardId), w)
export const clearBoardNotify = (boardId: string) => delJson<BoardNotifyAnswer>(boardPath(boardId))
export const getNotifyBoards = () => getJson<NotifyBoardsList>('/api/notifications/boards')
/** Set the default for every board, or clear it (null: the standard
 *  default). Answers the settings, whose `boards` is the default's state. */
export const saveBoardDefault = (w: BoardNotifyWrite | null) =>
  patchJson<{ boards?: BoardNotifyState }>('/api/notifications', { boards: w })

/** The query keys: one board's setting, and the Settings list. A write to
 *  either side invalidates the other. */
export const boardNotifyKey = (boardId: string) => ['board-notify', boardId] as const
export const NOTIFY_BOARDS_KEY = ['notify-boards'] as const

export function useBoardNotify(boardId: () => string | null, enabled: () => boolean) {
  return createQuery(() => {
    const id = boardId()
    return {
      queryKey: boardNotifyKey(id ?? ''),
      queryFn: () => getBoardNotify(id as string),
      enabled: !!id && enabled(),
      staleTime: 15_000,
    }
  })
}

export function useNotifyBoards(enabled: () => boolean) {
  return createQuery(() => ({
    queryKey: NOTIFY_BOARDS_KEY,
    queryFn: getNotifyBoards,
    enabled: enabled(),
    staleTime: 15_000,
  }))
}

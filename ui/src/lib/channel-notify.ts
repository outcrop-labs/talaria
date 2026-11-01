// A channel's notify level and timed mute, as the person sets them from the
// conversation header (components/chat/ChannelNotifyMenu.svelte).
//
// The server holds both on the caller's own membership row: `PUT
// /api/channels/{id}/notify` writes them, `GET /api/channels/{id}` serves
// them back as `notify: {level, mutedUntil, settable}`. The producers obey
// them (docs/notifications/producers.md): `all` files every message (a DM row
// per message, one counting `channel-message` row in a channel), `mentions`
// files @mentions and threads you are in, `none` files nothing — @mentions
// included. A running mute lands rows quietly: counted in the bell, never
// toasted, bannered or pushed.
//
// Framework-free so the rules are pinned by channel-notify.test.ts.

import { putJson } from './fetch-json'
import { FOREVER } from './notify-wire'

export type ChannelLevel = 'all' | 'mentions' | 'none'

/** What `GET /api/channels/{id}` serves as `notify`. */
export interface ChannelNotify {
  level: ChannelLevel
  /** ISO; null when no mute is running (an expired mute reads null). */
  mutedUntil: string | null
  /** False when the person has no member row to hold a level of their own —
   *  a task room (its roster is the board's) or a team grant. The PUT answers
   *  409 there, so the control is not offered. */
  settable: boolean
}

/** The body `PUT /api/channels/{id}/notify` takes: each member optional,
 *  `null` resets it (the level to the kind's default, the mute to none). */
export interface ChannelNotifyPatch {
  level?: ChannelLevel | null
  mutedUntil?: string | null
}

export const LEVELS: readonly ChannelLevel[] = ['all', 'mentions', 'none'] as const

/** The menu's words for each level. */
export function levelLabel(level: ChannelLevel): string {
  return level === 'all' ? 'All messages' : level === 'mentions' ? 'Mentions only' : 'Nothing'
}

/** The mute presets, in menu order. */
export type MutePreset = '1h' | '8h' | '24h' | 'forever'
export const MUTE_PRESETS: readonly MutePreset[] = ['1h', '8h', '24h', 'forever'] as const

/** The menu's words for each preset. */
export function muteLabel(preset: MutePreset): string {
  switch (preset) {
    case '1h':
      return 'Mute for 1 hour'
    case '8h':
      return 'Mute for 8 hours'
    case '24h':
      return 'Mute for 24 hours'
    case 'forever':
      return 'Mute until I turn it back on'
  }
}

/** The ISO instant a preset mutes until, from `nowMs`. */
export function muteUntil(preset: MutePreset, nowMs: number): string {
  if (preset === 'forever') return FOREVER
  const hours = preset === '1h' ? 1 : preset === '8h' ? 8 : 24
  return new Date(nowMs + hours * 3_600_000).toISOString()
}

/** A served `notify`, read defensively: an api that predates it (no `notify`
 *  in the detail) answers null and the control stays hidden — the degrade
 *  rule, a client gates on the server having the feature. */
export function readChannelNotify(detail: unknown): ChannelNotify | null {
  if (!detail || typeof detail !== 'object') return null
  const n = (detail as { notify?: unknown }).notify
  if (!n || typeof n !== 'object') return null
  const { level, mutedUntil, settable } = n as Record<string, unknown>
  if (typeof level !== 'string' || !(LEVELS as readonly string[]).includes(level)) return null
  return {
    level: level as ChannelLevel,
    mutedUntil: typeof mutedUntil === 'string' ? mutedUntil : null,
    settable: settable !== false,
  }
}

/** Whether a mute is running at `nowMs`. */
export function isMuted(n: Pick<ChannelNotify, 'mutedUntil'> | null, nowMs: number): boolean {
  if (!n?.mutedUntil) return false
  const t = Date.parse(n.mutedUntil)
  return Number.isFinite(t) && t > nowMs
}

/** The header's line for a running mute: "Muted until 3:15 PM", "Muted until
 *  Tue 9:00 AM" past today, "Muted until you unmute" for the forever mute.
 *  Null when nothing is muted. `locale`/`timeZone` are for the test. */
export function mutedLabel(
  n: Pick<ChannelNotify, 'mutedUntil'> | null,
  nowMs: number,
  opts: { locale?: string; timeZone?: string } = {},
): string | null {
  if (!isMuted(n, nowMs) || !n?.mutedUntil) return null
  if (n.mutedUntil === FOREVER) return 'Muted until you unmute'
  const at = new Date(n.mutedUntil)
  const time = at.toLocaleTimeString(opts.locale, { hour: 'numeric', minute: '2-digit', timeZone: opts.timeZone })
  const day = (d: Date) => d.toLocaleDateString('en-CA', { timeZone: opts.timeZone })
  if (day(at) === day(new Date(nowMs))) return `Muted until ${time}`
  const weekday = at.toLocaleDateString(opts.locale, { weekday: 'short', timeZone: opts.timeZone })
  return `Muted until ${weekday} ${time}`
}

/** Write the caller's level and/or mute, answering what the server stored. */
export async function putChannelNotify(channelId: string, patch: ChannelNotifyPatch): Promise<Pick<ChannelNotify, 'level' | 'mutedUntil'>> {
  const r = await putJson<{ ok: boolean; level: ChannelLevel; mutedUntil: string | null }>(
    `/api/channels/${encodeURIComponent(channelId)}/notify`,
    patch,
  )
  return { level: r.level, mutedUntil: r.mutedUntil }
}

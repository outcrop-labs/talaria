// The category × channel matrix (Settings → Notifications), as data. PURE.
//
// Its ROWS are the server's class registry, `classes[]` on GET
// /api/notifications (I-12) — never a list kept here: the registry owns every
// label, blurb, category and default, and members are not served the
// admin-only classes, so a member's matrix is shorter without this file
// knowing why. Its CELLS are `channels[class]` (I-13), the effective answer the
// server resolved (a stored bool, else what the legacy route meant, else the
// class default). A save is `PATCH {channels: {<class>: {<channel>: bool}}}`;
// the server rewrites the legacy route from it, so a rolled-back binary that
// reads only `prefs` behaves as close as it can.

import type { Category, Channels, ChannelsPatch, ChannelsPrefs, ClassInfo, EmailFrequency } from './notify-wire'

/** Category order on screen: what is blocked on you first. */
export const CATEGORY_ORDER: readonly Category[] = ['needs_you', 'direct', 'following', 'workspace']

export const CATEGORY_LABEL: Record<Category, string> = {
  needs_you: 'Needs you',
  direct: 'Direct',
  following: 'Following',
  workspace: 'Workspace',
}

export const CATEGORY_BLURB: Record<Category, string> = {
  needs_you: 'Work is waiting on you. These come through Do Not Disturb unless you say otherwise.',
  direct: 'Someone addressed you, or a conversation you are part of moved.',
  following: 'Outcomes and activity in things you follow.',
  workspace: 'The health of the workspace, for admins.',
}

export type MatrixChannel = 'desktop' | 'push' | 'email'

// What each column does, in the words a person checks a cell by — so each
// says what the engine does, not what it is called. Desktop is the row's
// `alert` (I-15): on, the row is `banner`, and the tab the person is looking
// at toasts it while an unwatched one banners it (notify-decide.ts rule 5
// and the tail); off, it is `feed`, and rule 2 shows NOTHING anywhere — the
// toast included, which the old "while you are not looking at Talaria"
// denied (notify-web-3).
// In-app's bell counts For you only (O-10), not "every notification".
export const MATRIX_COLUMNS: ReadonlyArray<{ id: 'inApp' | MatrixChannel; label: string; title: string }> = [
  { id: 'inApp', label: 'In-app', title: 'Always on: every notification is kept in your inbox, under the bell. Its badge counts Needs you and Direct.' },
  {
    id: 'desktop',
    label: 'Desktop',
    title:
      'The pop-up in Talaria while you are looking at it, and a banner on this computer (from a Talaria tab or the desktop app) while you are not. Off, neither shows: it waits in your inbox.',
  },
  { id: 'push', label: 'Push', title: 'A banner from your browser with every Talaria tab closed (Web Push).' },
  { id: 'email', label: 'Email', title: 'A missed-activity email, if you have not seen it in the app first.' },
]

export interface MatrixRow {
  cls: ClassInfo
  channels: Channels
  /** Every channel equals the registry default. */
  atDefault: boolean
}

export interface MatrixGroup {
  category: Category
  label: string
  blurb: string
  rows: MatrixRow[]
}

/** A class's effective channels: the server's answer, else its defaults. */
export function effectiveChannels(cls: ClassInfo, channels: ChannelsPrefs | undefined): Channels {
  return channels?.[cls.id] ?? { inApp: true, ...cls.defaults }
}

/** The optimistic overlay: what the person just clicked, over the server. */
export function overlay(ch: Channels, pending: ChannelsPatch[string] | undefined): Channels {
  return pending ? { ...ch, ...pending, inApp: true } : ch
}

export function atDefault(cls: ClassInfo, ch: Channels): boolean {
  return ch.desktop === cls.defaults.desktop && ch.push === cls.defaults.push && ch.email === cls.defaults.email
}

/** The matrix's rows, grouped by category in `CATEGORY_ORDER` (a category the
 *  client does not know yet goes last, under its own id), each group in
 *  registry order. Empty groups are left out. */
export function matrixGroups(
  classes: readonly ClassInfo[],
  channels: ChannelsPrefs | undefined,
  pending: ChannelsPatch = {},
): MatrixGroup[] {
  const order = [...CATEGORY_ORDER, ...classes.map((c) => c.category).filter((c) => !CATEGORY_ORDER.includes(c))]
  const groups: MatrixGroup[] = []
  for (const category of new Set(order)) {
    const rows = classes
      .filter((c) => c.category === category)
      .map((cls) => {
        const ch = overlay(effectiveChannels(cls, channels), pending[cls.id])
        return { cls, channels: ch, atDefault: atDefault(cls, ch) }
      })
    if (rows.length) {
      groups.push({
        category,
        label: CATEGORY_LABEL[category] ?? category,
        blurb: CATEGORY_BLURB[category] ?? '',
        rows,
      })
    }
  }
  return groups
}

/** One cell's save. */
export function setChannel(classId: string, channel: MatrixChannel, on: boolean): ChannelsPatch {
  return { [classId]: { [channel]: on } }
}

/** "Reset to default": every channel back to the registry's default. Written
 *  as explicit bools (the PATCH has no "unset"); the effect is the default. */
export function resetToDefault(cls: ClassInfo): ChannelsPatch {
  return { [cls.id]: { ...cls.defaults } }
}

/** Merge a second click into a pending overlay. */
export function mergePatch(a: ChannelsPatch, b: ChannelsPatch): ChannelsPatch {
  const out: ChannelsPatch = { ...a }
  for (const [k, v] of Object.entries(b)) out[k] = { ...out[k], ...v }
  return out
}

/** The legacy route a set of channels derives (I-13, both ways): email and a
 *  screen → `both`, email alone → `email`, no email → `in_app`. The server
 *  writes it; this is its mirror for the panel's "what an older app sees". */
export function legacyRoute(ch: Pick<Channels, 'desktop' | 'push' | 'email'>): 'in_app' | 'email' | 'both' {
  if (!ch.email) return 'in_app'
  return ch.desktop || ch.push ? 'both' : 'email'
}

export const EMAIL_FREQUENCY_OPTIONS: ReadonlyArray<{ id: EmailFrequency; label: string }> = [
  { id: '15m', label: '15 min' },
  { id: '1h', label: 'Hourly' },
  { id: '1d', label: 'Daily' },
  { id: 'off', label: 'Never' },
]

/** Why the Email column cannot be used, or null when it can. The instance
 *  switch's own explanation, the one the panel already shows above. */
export function emailColumnNote(emailEnabled: boolean): string | null {
  return emailEnabled ? null : 'Email delivery is off for this instance, so nothing in this column sends mail. Only an admin can turn it on.'
}

const listOf = (items: readonly string[]): string =>
  items.length < 2 ? (items[0] ?? '') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`

/** The missed-activity row's fine print — which rows wait their own time
 *  whatever the frequency, and what of Following is mailed — said from the
 *  registry and the person's effective channels, never kept as a sentence.
 *  The sentence it replaces, "Following is never mailed", sat over a
 *  registry whose Channel activity mails by default (classes.rs: a channel
 *  set to All gets what a DM gets), and the batcher mailed it under a
 *  FOLLOWING heading (notify-web-4). Same rule as the matrix above it: the
 *  registry owns every default, so the words follow it. */
export function mailNote(classes: readonly ClassInfo[], channels: ChannelsPrefs | undefined): string {
  const out: string[] = []
  // A class with its own delay waits it whatever the frequency (Needs you:
  // 15 minutes, deliver.rs email_due) — one sentence per category and delay.
  const delays = new Map<string, { category: Category; minutes: number }>()
  for (const c of classes) {
    if (c.emailDelayMin > 0) delays.set(`${c.category}:${c.emailDelayMin}`, { category: c.category, minutes: c.emailDelayMin })
  }
  for (const { category, minutes } of delays.values()) {
    out.push(`${CATEGORY_LABEL[category] ?? category} waits ${minutes} minute${minutes === 1 ? '' : 's'} whatever you pick.`)
  }
  const following = classes.filter((c) => c.category === 'following')
  const mailed = following.filter((c) => effectiveChannels(c, channels).email).map((c) => c.label)
  if (following.length > 0) {
    if (mailed.length === 0) out.push('Nothing under Following is mailed.')
    else if (mailed.length === following.length) out.push('Everything under Following is mailed too.')
    else out.push(`Under Following, only ${listOf(mailed)} ${mailed.length === 1 ? 'is' : 'are'} mailed.`)
  }
  return out.join(' ')
}

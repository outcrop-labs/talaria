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

export const MATRIX_COLUMNS: ReadonlyArray<{ id: 'inApp' | MatrixChannel; label: string; title: string }> = [
  { id: 'inApp', label: 'In-app', title: 'Always on: every notification is kept in your inbox and counted on the bell.' },
  { id: 'desktop', label: 'Desktop', title: 'A banner on this computer, from a Talaria tab or the desktop app, while you are not looking at Talaria.' },
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

// The Board settings modal's tabs, and the `?settings=<tab>` deep link on the
// board route that opens it (UI-CONVENTIONS "Deep links"): the open tab is a
// selection a person can share or step back through, so it lives in the URL.
// Board.svelte reads the param, BoardSettingsModal.svelte draws the tab, and a
// door elsewhere on the board (the sync chip's "Sync settings") writes the
// param instead of reaching into the modal.

export const SETTINGS_TABS = ['general', 'statuses', 'labels', 'people', 'agents', 'sync', 'notifications'] as const
export type SettingsTab = (typeof SETTINGS_TABS)[number]

/** The tabs a board role sees, in strip order. Owners and editors configure
 *  the board, so they see every tab. A viewer sees only their own
 *  notification setting: the board's configuration is not theirs to change
 *  (the api answers a viewer's writes 403), and the General and People tabs
 *  are not read-only views. */
export function tabsFor(role: string | null | undefined): readonly SettingsTab[] {
  if (role === 'owner' || role === 'editor') return SETTINGS_TABS
  if (role === 'viewer') return ['notifications']
  return []
}

/** Whether Board settings opens for this role on this tab: only on a tab the
 *  role sees — a link to a tab someone cannot see opens nothing, as an
 *  unknown tab does. */
export function opensOn(role: string | null | undefined, tab: SettingsTab | null | undefined): boolean {
  return !!tab && tabsFor(role).includes(tab)
}

/** The tab a `?settings=` value names, or null when it names none — an unknown
 *  value opens nothing rather than a guess. */
export function settingsTabOf(v: unknown): SettingsTab | null {
  return typeof v === 'string' && (SETTINGS_TABS as readonly string[]).includes(v) ? (v as SettingsTab) : null
}

/** The strip's words for a tab (the cells render uppercase). */
export const SETTINGS_TAB_LABEL: Record<SettingsTab, string> = {
  general: 'general',
  statuses: 'statuses',
  labels: 'labels',
  people: 'people',
  agents: 'agents',
  sync: 'sync',
  notifications: 'notifications',
}

/** The query string a ticket opened from inside the board's settings or the
 *  sync chip carries: the board's CURRENT view and filters (Board.svelte's
 *  rule — the ticket overlays the view, and closing it returns there), minus
 *  `settings`, so the modal closes under the ticket rather than reopening on
 *  top of it. */
export function ticketSearch(search: string): string {
  const sp = new URLSearchParams(search)
  sp.delete('settings')
  return sp.toString()
}

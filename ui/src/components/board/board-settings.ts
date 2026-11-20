// The Board settings modal's tabs, and the `?settings=<tab>` deep link on the
// board route that opens it (UI-CONVENTIONS "Deep links"): the open tab is a
// selection a person can share or step back through, so it lives in the URL.
// Board.svelte reads the param, BoardSettingsModal.svelte draws the tab, and a
// door elsewhere on the board (the sync chip's "Sync settings") writes the
// param instead of reaching into the modal.

export const SETTINGS_TABS = ['general', 'statuses', 'labels', 'people', 'agents', 'sync'] as const
export type SettingsTab = (typeof SETTINGS_TABS)[number]

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
}

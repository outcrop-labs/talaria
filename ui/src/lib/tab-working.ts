// Which tabs draw a waiting glyph, and what the bar's "N working" readout
// says, from one /api/working read. A tab maps to an arm by its route, never
// by NavItem.badge: Work's badge key is `sessions` and Boards has none.
import { isUnder } from './route-tabs'
import type { WaitingSiteKey } from './waiting/sites'
import type { Lead, Working } from './working.svelte'

const ARMS: readonly { arm: keyof Working; base: string; site: WaitingSiteKey }[] = [
  { arm: 'comms', base: '/comms', site: 'nav/tab-comms' },
  { arm: 'work', base: '/work', site: 'nav/tab-work' },
  { arm: 'boards', base: '/boards', site: 'nav/tab-boards' },
]

export interface TabWorking {
  site: WaitingSiteKey
  lead: Lead
}

/** Null when the tab's view has no arm, its arm is idle or absent, or the
 *  read failed. A null `working` is a failed read, never "nothing working". */
export function tabWorking(to: string, working: Working | null): TabWorking | null {
  if (!working) return null
  const row = ARMS.find((a) => isUnder(to, a.base))
  const lead = row ? (working[row.arm] ?? null) : null
  return row && lead ? { site: row.site, lead } : null
}

export interface Readout<T> {
  /** Views working other than the open one. */
  count: number
  /** The first of them in bar order: its tab, glyph, label and link. */
  tab: T
  site: WaitingSiteKey
  lead: Lead
  /** One line per working view, for the tooltip. */
  title: string
}

/** The readout over the tabs the bar shows, in bar order. A view the viewer
 *  has no tab for never counts. */
export function readout<T extends { to: string; label: string }>(
  tabs: readonly T[],
  working: Working | null,
  openTo: string | null,
): Readout<T> | null {
  const lit = tabs.flatMap((tab) => {
    if (tab.to === openTo) return []
    const w = tabWorking(tab.to, working)
    return w ? [{ tab, ...w }] : []
  })
  const first = lit[0]
  if (!first) return null
  return {
    count: lit.length,
    ...first,
    title: lit.map((l) => `${l.tab.label}: ${l.lead.label}`).join('\n'),
  }
}

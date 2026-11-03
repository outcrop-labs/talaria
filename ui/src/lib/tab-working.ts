import { isUnder } from './route-tabs'
import type { WaitingSiteKey } from './waiting/sites'
import type { Lead, Working } from './working.svelte'

// `/plan` and `/research` still render the Work harness (route-table.ts), so
// being there is being in Work even though no tab lights for them.
const ARMS: readonly { arm: keyof Working; bases: readonly string[]; site: WaitingSiteKey }[] = [
  { arm: 'comms', bases: ['/comms'], site: 'nav/tab-comms' },
  { arm: 'work', bases: ['/work', '/plan', '/research'], site: 'nav/tab-work' },
  { arm: 'boards', bases: ['/boards'], site: 'nav/tab-boards' },
]

const armAt = (path: string) => ARMS.find((a) => a.bases.some((b) => isUnder(path, b)))

export interface TabWorking {
  site: WaitingSiteKey
  lead: Lead
}

export function tabWorking(to: string, working: Working | null): TabWorking | null {
  if (!working) return null
  const row = armAt(to)
  const lead = row ? (working[row.arm] ?? null) : null
  return row && lead ? { site: row.site, lead } : null
}

export interface Readout<T> {
  elsewhere: number
  tab: T
  site: WaitingSiteKey
  lead: Lead
  title: string
}

export function readout<T extends { to: string; label: string }>(
  tabs: readonly T[],
  working: Working | null,
  pathname: string,
): Readout<T> | null {
  const here = armAt(pathname)
  const lit = tabs.flatMap((tab) => {
    if (here && armAt(tab.to) === here) return []
    const w = tabWorking(tab.to, working)
    return w ? [{ tab, ...w }] : []
  })
  const first = lit[0]
  if (!first) return null
  return {
    elsewhere: lit.length,
    ...first,
    title: lit.map((l) => `${l.tab.label}: ${l.lead.label}`).join('\n'),
  }
}

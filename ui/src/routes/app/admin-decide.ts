// The decision model as Admin → Agents shows it: the wire shapes of
// /api/admin/decide, and what each site's row and detail say about them.
import type { Fact } from '@/components/admin/facts'

export interface Provider {
  id: string
  label: string
  country: string
  needsUrl: boolean
  needsKey: boolean
  needsModel: boolean
  needsWire: boolean
  wire: string
  fallbackModels: string[]
  liveCatalog: boolean
  primitives: string[]
  fansOut: boolean
  calibration: string
}

// THE CENSUS — every place in Talaria that asks a decision model anything.
// Listed in full, including the sites this panel cannot switch: "where is a
// decision model being used" has to have one complete answer, or the answer
// is "nobody is sure".
export interface Site {
  id: string
  label: string
  primitive: string
  acts: string
  on: boolean
  floor: number
  defaultOn: boolean
  defaultFloor: number
  reads: string
  floorWhy: string
  switchLivesAt: string | null
  /** Where this site's numbers live when they are not in the shadow ledger.
   *  A guard finding IS the output, so there is no second answer to compare
   *  it against — a row that said "no comparisons yet" for ever would read
   *  as a bug rather than as the truth. */
  measuredAt: string | null
}

export interface SiteShadow {
  site: string
  compared: number
  acted: number
  answered: number
  agreed: number
  judgments: number
  calibrated: number
  models: Array<{ model: string; judgments: number }>
  tokensIn: number
  metered: number
  p50Ms: number | null
  p99Ms: number | null
  meanCertaintyWhenDisagreed: number | null
  provider: string | null
  newest: string | null
}

export interface DecideData {
  providers: Provider[]
  wires: string[]
  config: {
    provider: string
    url?: string | null
    model?: string | null
    wire?: string | null
    timeoutMs?: number | null
    hasKey: boolean
  }
  configured: boolean
  shadow: SiteShadow[]
  sites: Site[]
  speechRooms: Array<{ id: string; name: string; agents: number }>
}

export interface LedgerRow {
  subjectRef: string | null
  baseline: string | null
  portAnswer: string | null
  certainty: number | null
  calibrated: boolean
  model: string | null
  latencyMs: number | null
  agreed: boolean | null
  at: string | null
}

export const PRIMITIVE_LABEL: Record<string, string> = {
  noul: 'Yes or no',
  choice: 'Pick one',
  score: 'How much',
}

const num = (n: number) => n.toLocaleString()
export const pct = (n: number, d: number) => (d > 0 ? `${Math.round((n / d) * 100)}%` : '—')

/** A site's row status. Two models at one site outranks everything else,
 *  because it makes the agreement beside it describe neither of them. */
export function siteStatus(s: Site, m: SiteShadow | undefined): { status: 'ok' | 'warn' | 'idle'; word: string } {
  if (s.switchLivesAt) return { status: 'idle', word: 'Set elsewhere' }
  if (m && m.models.length > 1) return { status: 'warn', word: 'Mixed models' }
  if (s.on) return { status: 'ok', word: 'Deciding' }
  if (m && m.compared > 0) return { status: 'idle', word: 'Comparing' }
  return { status: 'idle', word: 'Off' }
}

/** What a site's own traffic measured, as facts. Decisions it made alone are
 *  counted apart from the comparisons: once a site is switched over there is
 *  no second answer, and folding them in would read as 100% agreement. */
export function siteFacts(m: SiteShadow): Fact[] {
  const facts: Fact[] = []
  if (m.compared > 0) {
    const silent = m.compared > m.answered ? `, silent on ${num(m.compared - m.answered)} of ${num(m.compared)}` : ''
    facts.push({ label: 'Agreement', value: `${pct(m.agreed, m.answered)} of ${num(m.answered)} answered${silent}` })
  }
  if (m.p50Ms !== null) facts.push({ label: 'Speed', value: `${num(m.p50Ms)} ms median, ${num(m.p99Ms ?? 0)} ms at the 99th percentile` })
  if (m.acted > 0) facts.push({ label: 'Decided alone', value: `${num(m.acted)} ${m.acted === 1 ? 'turn' : 'turns'} since it was turned on` })
  const only = m.models.length === 1 ? m.models[0] : undefined
  if (only) facts.push({ label: 'Answered by', value: only.model, mono: true })
  const total = m.compared + m.acted
  if (m.metered > 0) {
    const unmetered = m.metered < total ? `, ${num(total - m.metered)} reported none` : ''
    facts.push({ label: 'Cost', value: `${num(m.tokensIn)} input tokens over ${num(m.metered)} ${m.metered === 1 ? 'call' : 'calls'}${unmetered}` })
  } else if (total > 0) {
    facts.push({ label: 'Cost', value: 'This provider reports no token usage' })
  }
  return facts
}

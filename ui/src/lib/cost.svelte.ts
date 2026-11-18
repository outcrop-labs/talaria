// Token-ledger client.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface CostTotals {
  prompt: number
  completion: number
  /** Cache writes + reads — billed at their own multipliers, not 1x. */
  cache?: number
  generations: number
  /** Dollars (local = $0; unpriced cloud excluded — see unpricedCloudTokens). */
  cost?: number
}

export interface CostOverview {
  totals: {
    today: CostTotals
    week: CostTotals
    month: CostTotals
    estimatedShare: number
    split: { local: number; cloud: number; other: number }
    unpricedCloudTokens: number
    /** Cloud tokens an admin marked included in a plan: priced at $0, and
     *  labelled wherever spend is shown — never a silent zero. */
    includedCloudTokens?: number
    /** Of the 30-day dollars, those estimated from a published price rather
     *  than charged by the provider or set by an admin. */
    derivedCost?: number
    providerReported?: { cost: number; source: string; fetchedAt: string | null; stale: boolean }
  }
  perModel: Array<{
    llmModel: string | null
    endpointClass: 'local' | 'cloud' | null
    tokens: number
    cost: number | null
    /** Of `tokens`, those included in a plan ($0, labelled). */
    includedTokens?: number
    /** The catalog entry a FUZZY match priced the model as. */
    pricedAs?: string
    matchKind?: string
  }>
  perAgent: Array<
    CostTotals & { agentModel: string; lastUsed: string | null; cost: number; localShare: number | null; includedTokens?: number }
  >
  perDay: Array<CostTotals & { day: string; local: number; cloud: number }>
}

export function useCost() {
  return createQuery(() => ({
    queryKey: ['cost'],
    queryFn: (): Promise<CostOverview> => getJson<CostOverview>('/api/cost'),
    refetchInterval: 60_000,
  }))
}

/** Client-safe split of an agent id "<slug>-<department>" into label + role. */
export function agentLabel(id: string): { label: string; role: string } {
  const [first, ...rest] = id.split('-')
  const label = first ? first.charAt(0).toUpperCase() + first.slice(1) : id
  return { label, role: rest.join(' ') }
}

// The dollar and token spellings are shared now — `@/lib/format` — and
// re-exported under the names every cost surface already imports them by.
// `formatCost` is `formatUsd`'s DEFAULT answer: a sub-cent spend keeps four
// decimals, so a real amount never renders as "$0.00". The model surfaces take
// its `unpricedAsWord` variant instead (see `@/components/models/fitness`).
// Every caller is in the settings area, so thousands are grouped.
export { formatTokens } from '@/lib/format'
import { formatTokens as formatTokensLabel, formatUsd } from '@/lib/format'
export const formatCost = (n: number | null): string => formatUsd(n, { group: true })
const formatUsdLabel = formatCost

/** A cloud model's spend as the legend says it: "unpriced", "included in a
 *  plan" when every token is, the dollars plus how much was included when
 *  some is — an included model never reads as a bare "$0". */
export function modelCostLabel(m: { cost: number | null; tokens: number; includedTokens?: number; pricedAs?: string }): string {
  const included = m.includedTokens ?? 0
  if (m.cost === null) return 'unpriced'
  if (included > 0 && included >= m.tokens) return 'included in a plan'
  const dollars = included > 0 ? `${formatUsdLabel(m.cost)} + ${formatTokensLabel(included)} included` : formatUsdLabel(m.cost)
  return m.pricedAs ? `${dollars}, priced as ${m.pricedAs} (fuzzy match)` : dollars
}

/** An agent's 30-day dollars as the "By agent" list says them — a run
 *  entirely on an included plan reads "included", never a bare $0. */
export function agentCostLabel(a: { cost: number; prompt: number; completion: number; includedTokens?: number }): string {
  const included = a.includedTokens ?? 0
  if (included > 0 && a.cost === 0) return 'included'
  return formatUsdLabel(a.cost)
}

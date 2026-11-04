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
    providerReported?: { cost: number; source: string; fetchedAt: string | null; stale: boolean }
  }
  perModel: Array<{
    llmModel: string | null
    endpointClass: 'local' | 'cloud' | null
    tokens: number
    cost: number | null
    /** Of `tokens`, those included in a plan ($0, labelled). */
    includedTokens?: number
  }>
  perAgent: Array<CostTotals & { agentModel: string; lastUsed: string | null; cost: number; localShare: number | null }>
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
export { formatTokens, formatUsd as formatCost } from '@/lib/format'
import { formatTokens as formatTokensLabel, formatUsd as formatUsdLabel } from '@/lib/format'

/** A cloud model's spend as the legend says it: "unpriced", "included in a
 *  plan" when every token is, the dollars plus how much was included when
 *  some is — an included model never reads as a bare "$0". */
export function modelCostLabel(m: { cost: number | null; tokens: number; includedTokens?: number }): string {
  const included = m.includedTokens ?? 0
  if (m.cost === null) return 'unpriced'
  if (included > 0 && included >= m.tokens) return 'included in a plan'
  if (included > 0) return `${formatUsdLabel(m.cost)} + ${formatTokensLabel(included)} included`
  return formatUsdLabel(m.cost)
}

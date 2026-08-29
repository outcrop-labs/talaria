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
  }
  perModel: Array<{ llmModel: string | null; endpointClass: 'local' | 'cloud' | null; tokens: number; cost: number | null }>
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

/** $12.3456 → "$12.35"; tiny amounts keep enough precision to be non-zero. */
export function formatCost(n: number): string {
  if (n === 0) return '$0'
  if (n < 0.01) return `$${n.toFixed(4)}`
  return `$${n.toFixed(2)}`
}

/** 1234 → "1.2k", 5_600_000 → "5.6M". */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}k`
  return String(n)
}

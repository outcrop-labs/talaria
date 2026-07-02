// Token-ledger client. Dollar cost lands once per-LLM pricing attribution exists.
import { useQuery } from '@tanstack/react-query'

export interface CostTotals {
  prompt: number
  completion: number
  generations: number
}

export interface CostOverview {
  totals: { today: CostTotals; week: CostTotals; month: CostTotals; estimatedShare: number }
  perAgent: Array<CostTotals & { agentModel: string; lastUsed: string | null }>
  perDay: Array<CostTotals & { day: string }>
}

export function useCost() {
  return useQuery({
    queryKey: ['cost'],
    queryFn: async (): Promise<CostOverview | null> => {
      const r = await fetch('/api/cost', { credentials: 'same-origin' })
      if (!r.ok) return null
      return r.json()
    },
    refetchInterval: 60_000,
  })
}

/** Client-safe "dex-developer" → { label: "Dex", role: "developer" }. */
export function agentLabel(id: string): { label: string; role: string } {
  const [first, ...rest] = id.split('-')
  const label = first ? first.charAt(0).toUpperCase() + first.slice(1) : id
  return { label, role: rest.join(' ') }
}

/** 1234 → "1.2k", 5_600_000 → "5.6M". */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}k`
  return String(n)
}

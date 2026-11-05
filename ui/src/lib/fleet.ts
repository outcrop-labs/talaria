import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

/** Container reality: `idle` while the agent's container runs, `offline`
 *  otherwise — the same source as the roster's status dots. */
export type AgentStatus = 'offline' | 'idle'

export interface FleetAgentStat {
  id: string
  label: string
  role: string
  status: AgentStatus
  conversations: number
  messages: number
  lastUsed: string | null
}

export interface FleetOverview {
  agents: FleetAgentStat[]
  totals: { agents: number; online: number; conversations: number; messages: number; activeToday: number }
}

export function useFleet() {
  return createQuery(() => ({
    queryKey: ['fleet'],
    queryFn: (): Promise<FleetOverview> => getJson<FleetOverview>('/api/fleet'),
    refetchInterval: 15_000,
  }))
}

/** How far off an epoch-ms moment in the future is, as a panel says it
 *  ("within a minute", "in 12 min", "in about 3 h"); null for none. */
export function untilTime(ms: number | null | undefined): string | null {
  if (!ms) return null
  const mins = Math.round((ms - Date.now()) / 60_000)
  if (mins <= 1) return 'within a minute'
  if (mins < 90) return `in ${mins} min`
  return `in about ${Math.round(mins / 60)} h`
}

/** "3m ago" / "2h ago" / "just now" — compact relative time. */
export function relativeTime(iso: string | null): string {
  if (!iso) return 'never'
  const diff = Date.now() - new Date(iso).getTime()
  const s = Math.floor(diff / 1000)
  if (s < 60) return 'just now'
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.floor(h / 24)}d ago`
}

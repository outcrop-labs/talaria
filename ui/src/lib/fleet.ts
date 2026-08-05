import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export type AgentStatus = 'offline' | 'idle' | 'busy' | 'error'

export interface FleetAgentStat {
  id: string
  label: string
  role: string
  status: AgentStatus
  lastSeen: string | null
  lastActivity: string | null
  conversations: number
  messages: number
  lastUsed: string | null
}

export interface FleetOverview {
  agents: FleetAgentStat[]
  source: 'gateway' | 'mock'
  totals: { agents: number; online: number; conversations: number; messages: number; activeToday: number }
}

export function useFleet() {
  return createQuery(() => ({
    queryKey: ['fleet'],
    queryFn: (): Promise<FleetOverview> => getJson<FleetOverview>('/api/fleet'),
    refetchInterval: 15_000,
  }))
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

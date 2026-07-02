import { useQuery } from '@tanstack/react-query'

export interface AgentModel {
  id: string
  label: string
  role: string
  /** Requestable model tiers (alias names); the main model is implicit. */
  tiers?: string[]
}

export function useAgents() {
  return useQuery({
    queryKey: ['agents'],
    queryFn: async (): Promise<{ agents: AgentModel[]; source: 'gateway' | 'mock' }> => {
      const res = await fetch('/api/agents', { credentials: 'same-origin' })
      if (!res.ok) return { agents: [], source: 'mock' }
      return res.json()
    },
    staleTime: 30_000,
  })
}

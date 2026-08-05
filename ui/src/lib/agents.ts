import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface AgentModel {
  id: string
  label: string
  role: string
  /** Requestable model tiers (alias names); the main model is implicit. */
  tiers?: string[]
}

export function useAgents() {
  return createQuery(() => ({
    queryKey: ['agents'],
    // `source: 'mock'` describes a gateway that answered without a real fleet.
    // A failed request is not a mock fleet — it is a failure.
    queryFn: (): Promise<{ agents: AgentModel[]; source: 'gateway' | 'mock' }> =>
      getJson<{ agents: AgentModel[]; source: 'gateway' | 'mock' }>('/api/agents'),
    staleTime: 30_000,
  }))
}

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
    // An empty list means an empty fleet. A failed request is not an empty
    // fleet — it is a failure, and getJson throws so it surfaces as one.
    queryFn: (): Promise<{ agents: AgentModel[] }> => getJson<{ agents: AgentModel[] }>('/api/agents'),
    staleTime: 30_000,
  }))
}

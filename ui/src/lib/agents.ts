import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface AgentModel {
  id: string
  label: string
  role: string
  /** Requestable model tiers (alias names); the main model is implicit. */
  tiers?: string[]
  /** The agent keeps its conversations as Hermes sessions, which always run
   *  its main model — see `PINNED_TIER_NOTE`. */
  pinnedSessions?: boolean
}

/** Why the tier chip is off for an agent with pinned sessions. Hermes fixes a
 *  session's model at its first turn and Talaria's session turn carries no
 *  model, so a tier picked for one message would not run; the reply would
 *  come from the main model either way. */
export const PINNED_TIER_NOTE =
  "This agent keeps pinned sessions, which always run its main model: Hermes fixes a session's model at its first turn, so a tier would not apply."

export function useAgents() {
  return createQuery(() => ({
    queryKey: ['agents'],
    // An empty list means an empty fleet. A failed request is not an empty
    // fleet — it is a failure, and getJson throws so it surfaces as one.
    queryFn: (): Promise<{ agents: AgentModel[] }> => getJson<{ agents: AgentModel[] }>('/api/agents'),
    staleTime: 30_000,
  }))
}

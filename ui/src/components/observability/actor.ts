import { agentLabel } from '@/lib/cost.svelte'

/** Who an activity row names. The api serves an agent as its slug and a
 *  person as their address or name: a slug reads as the agent's name
 *  ("relay" is "Relay"), and anything with an @ or a space is left as served. */
export const actorLabel = (actor: string): string => (/[@\s]/.test(actor) ? actor : agentLabel(actor).label)

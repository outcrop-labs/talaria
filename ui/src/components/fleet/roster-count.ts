import { containersOf, type AgentContainers, type AgentDef } from '@/lib/fleet-defs'
import { HEALTH_LABEL, healthOf, type Health } from '@/routes/app/agents.svelte'

/** States the count is out of: an agent that is meant to be up. A draft was
 *  never started and a retired agent never will be, so neither is counted. */
const COUNTED = ['up', 'warming', 'degraded', 'down'] as const satisfies readonly Health[]
/** Counted agents that are not running, in the order they are read out. */
const EXCEPTIONS = ['down', 'degraded', 'warming'] as const satisfies readonly Health[]
const UNCOUNTED = ['draft', 'retired'] as const satisfies readonly Health[]

export interface RosterCount {
  tally: Record<Health, number>
  /** Agents that are up and healthy. */
  running: number
  /** What `running` is out of. */
  counted: number
  /** The counted agents that are not running: "1 unhealthy", "1 starting". */
  exceptions: string[]
  /** The agents on the roster the count leaves out: "1 draft", "1 retired". */
  uncounted: string[]
}

const phrases = (tally: Record<Health, number>, states: readonly Health[]): string[] =>
  states.filter((h) => tally[h] > 0).map((h) => `${tally[h]} ${HEALTH_LABEL[h].toLowerCase()}`)

/** How much of a roster is running, from container reality. The one
 *  definition: the roster's line reads it, and the overview's tile can. */
export function rosterCount(defs: readonly AgentDef[], byDept: Map<string, AgentContainers>): RosterCount {
  const tally: Record<Health, number> = { up: 0, warming: 0, degraded: 0, down: 0, draft: 0, retired: 0 }
  for (const d of defs) tally[healthOf(d, containersOf(d, byDept)).health] += 1
  return {
    tally,
    running: tally.up,
    counted: COUNTED.reduce((n, h) => n + tally[h], 0),
    exceptions: phrases(tally, EXCEPTIONS),
    uncounted: phrases(tally, UNCOUNTED),
  }
}

/** The count as the roster says it: "5 of 7 running", then each exception,
 *  then who the count leaves out. Empty for a roster with nobody on it. */
export function rosterLine(c: RosterCount): string[] {
  const parts = c.counted > 0 ? [`${c.running} of ${c.counted} running`, ...c.exceptions] : c.uncounted.length > 0 ? ['None running'] : []
  if (c.uncounted.length > 0) parts.push(`${c.uncounted.join(' and ')} not counted`)
  return parts
}

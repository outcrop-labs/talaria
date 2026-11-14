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

/** Each state that has agents in it, as the roster says it: "1 unhealthy". */
export const healthPhrases = (tally: Record<Health, number>, states: readonly Health[]): string[] =>
  states.filter((h) => tally[h] > 0).map((h) => `${tally[h]} ${HEALTH_LABEL[h].toLowerCase()}`)

/** How much of a roster is running, from container reality. The one
 *  definition: the roster's line and the overview's tile both read it. */
export function rosterCount(defs: readonly AgentDef[], byDept: Map<string, AgentContainers>): RosterCount {
  const tally: Record<Health, number> = { up: 0, warming: 0, degraded: 0, down: 0, draft: 0, retired: 0 }
  for (const d of defs) tally[healthOf(d, containersOf(d, byDept)).health] += 1
  return {
    tally,
    running: tally.up,
    counted: COUNTED.reduce((n, h) => n + tally[h], 0),
    exceptions: healthPhrases(tally, EXCEPTIONS),
    uncounted: healthPhrases(tally, UNCOUNTED),
  }
}

/** The count as the roster says it: "5 of 7 running", then each exception.
 *  Empty for a roster with nobody on it. */
export function rosterLine(c: RosterCount): string[] {
  if (c.counted > 0) return [`${c.running} of ${c.counted} running`, ...c.exceptions]
  return c.uncounted.length > 0 ? ['None running'] : []
}

const LEFT_OUT: Partial<Record<Health, string>> = { draft: 'drafts', retired: 'retired agents' }

/** What the line leaves for its tip, in sentences: who the count leaves out,
 *  and how many agents were active today when that read has landed. Null
 *  when there is nothing to add. */
export function rosterNote(c: RosterCount, activeToday?: number): string | null {
  const out = UNCOUNTED.filter((h) => c.tally[h] > 0).map((h) => LEFT_OUT[h] ?? h)
  const sentences: string[] = []
  if (out.length > 0) {
    const who = out.join(' and ')
    sentences.push(`${who.charAt(0).toUpperCase()}${who.slice(1)} are not counted.`)
  }
  if (activeToday !== undefined) {
    sentences.push(activeToday === 0 ? 'None were active today.' : `${activeToday} ${activeToday === 1 ? 'was' : 'were'} active today.`)
  }
  return sentences.length > 0 ? sentences.join(' ') : null
}

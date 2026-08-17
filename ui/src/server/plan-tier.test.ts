// The one thing that survived `plan-persona-turn.ts`.
//
// That file was a whole persona transport written by hand so the two plan
// harnesses could route a TIER id and attribute the spend correctly. Both are
// features of `runHarness` now — `pickTransport` classifies a tier, and
// `RunContext.tier` builds the routed id AND names the alias for the ledger — so
// all the adapters have left to do is hand over the two names separately.
//
// This is the arithmetic that separates them, and it is tested because its
// failure is silent rather than loud. `recordUsage` prices a row by finding
// `agent_defs.model = agentModel` and then the alias named by `tier`. Get it
// wrong and BOTH lookups miss: the row lands on an agent that does not exist,
// with no endpoint class, which means no price. A plan drafted on the expensive
// tier is metered at zero and nothing anywhere says so.
import { describe, expect, it } from 'vitest'
import { planTier } from './plan-doc'

/** `routedModelFor` in fleet-agents.ts, verbatim — the only producer of the
 *  pairs `planTier` is ever handed. Inlined rather than imported so this test
 *  needs no fleet: what is being asserted is that the two functions are
 *  inverses, which is a statement about the RULE, not about a database. */
const routedModelFor = (agentModel: string, tier: string | null): string => (tier ? `${agentModel}-${tier}` : agentModel)

describe('planTier', () => {
  it('is null when no tier was picked, so the run is metered against the agent itself', () => {
    // Both plan routes fall back to the bare agent when the modal's tier
    // dropdown is untouched or the pick fails validation.
    expect(planTier('engineer-engineering', 'engineer-engineering')).toBeNull()
  })

  it('recovers the alias NAME, which is what the ledger prices from', () => {
    expect(planTier('engineer-engineering', 'engineer-engineering-opus')).toBe('opus')
  })

  it('inverts routedModelFor for every shape the fleet can produce', () => {
    // Agent ids are themselves `<handle>-<department>`, so the hyphen the alias
    // is joined with is never the only one in the string — splitting at the
    // FIRST hyphen, or at the last, would both be wrong here. Anchoring on the
    // known agent id is the only reading that works.
    const cases: Array<[string, string | null]> = [
      ['engineer-engineering', 'opus'],
      ['engineer-engineering', null],
      ['assistant-operations', 'haiku'],
      ['analyst', 'sonnet-4-5'],
      ['a-b-c-d', 'fast'],
    ]
    for (const [agent, tier] of cases) {
      expect(planTier(agent, routedModelFor(agent, tier))).toBe(tier)
    }
  })

  it('round-trips back to the id that goes on the wire', () => {
    // `runHarness` reassembles `${model}-${tier}` and calls THAT. If this pair
    // did not round-trip, the plan would be drafted by a different model than
    // the user picked — which is the loud half of the same bug.
    const agent = 'engineer-engineering'
    const routed = 'engineer-engineering-opus'
    const tier = planTier(agent, routed)
    expect(tier === null ? agent : `${agent}-${tier}`).toBe(routed)
  })
})

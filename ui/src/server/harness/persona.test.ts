import { describe, expect, it } from 'vitest'
import { personaEffortIndex, personaIndex, personaKeysFrom, personaTargets, type PersonaRow } from '@/server/harness/persona'

// The resolver's whole job is to answer "which endpoint:model is actually
// behind this persona id" — and to answer NOTHING rather than guess. These
// cases are the defensive half: an `agent_versions.config` row is jsonb that
// outlives the code that wrote it, so a hand-edited or half-migrated one has to
// land a harness on the unknown path instead of crashing a run or, worse,
// producing a key that pools another model's facts.
//
// The behavioral half (widening, the floor, unanimity) is asserted end to end
// through the runner in run.test.ts, where it belongs.

const penny: PersonaRow = {
  agent: 'assistant-operations',
  config: {
    main: { endpoint: 'spark', model: 'qwen3-14b' },
    aliases: [
      { name: 'opus', endpoint: 'anthropic', model: 'claude-opus-4' },
      { name: 'fast', endpoint: 'spark', model: 'qwen3-4b' },
    ],
    fallbacks: [{ endpoint: 'thunder', model: 'llama3-8b' }],
  },
}

describe('resolving a persona to its backing targets', () => {
  it('maps the base id to main plus the fallback chain', () => {
    expect(personaTargets('assistant-operations', [penny])).toEqual([
      { endpoint: 'spark', model: 'qwen3-14b' },
      { endpoint: 'thunder', model: 'llama3-8b' },
    ])
  })

  it('maps every declared tier to its own target', () => {
    expect(personaKeysFrom('assistant-operations-opus', [penny])).toEqual(['anthropic:claude-opus-4', 'thunder:llama3-8b'])
    expect(personaKeysFrom('assistant-operations-fast', [penny])).toEqual(['spark:qwen3-4b', 'thunder:llama3-8b'])
  })

  it('answers nothing for an unknown id or an undeclared tier', () => {
    expect(personaKeysFrom('assistant-operations-turbo', [penny])).toEqual([])
    expect(personaKeysFrom('nobody-here', [penny])).toEqual([])
    expect(personaKeysFrom('penny', [penny])).toEqual([])
  })

  it('deduplicates a fallback that points at the same place as the tier', () => {
    // Otherwise the same endpoint:model would be asked twice on the hot path and
    // counted twice in a unanimity vote.
    const rows: PersonaRow[] = [
      { agent: 'engineer-engineering', config: { main: { endpoint: 'spark', model: 'q' }, fallbacks: [{ endpoint: 'spark', model: 'q' }] } },
    ]
    expect(personaKeysFrom('engineer-engineering', rows)).toEqual(['spark:q'])
  })

  it('refuses to stand in the fallbacks for a missing main', () => {
    // A fallback only serves when the primary fails, so a pool that omits the
    // primary is a claim about a model that will usually not be answering.
    const rows: PersonaRow[] = [{ agent: 'engineer-engineering', config: { fallbacks: [{ endpoint: 'thunder', model: 'llama3-8b' }] } }]
    expect(personaTargets('engineer-engineering', rows)).toEqual([])
  })

  it('drops a half-written target rather than keying on a blank half', () => {
    // "spark:" is a key any other half-written row could also produce, so the
    // two would silently pool each other's facts.
    const rows: PersonaRow[] = [
      { agent: 'a-one', config: { main: { endpoint: 'spark' } } },
      { agent: 'b-two', config: { main: { endpoint: '', model: 'q' } } },
      { agent: 'c-three', config: { main: { endpoint: 'spark', model: 'q' }, aliases: [{ name: 'big' }] } },
    ]
    expect(personaTargets('a-one', rows)).toEqual([])
    expect(personaTargets('b-two', rows)).toEqual([])
    expect(personaTargets('c-three-big', rows)).toEqual([])
  })

  it('survives config shapes no version of this code ever wrote', () => {
    const rows: PersonaRow[] = [
      { agent: 'a-one', config: null },
      { agent: 'b-two', config: 'main: spark' },
      { agent: 'c-three', config: [] },
      { agent: 'd-four', config: { main: 'spark/qwen', aliases: 'opus', fallbacks: 'thunder' } },
    ]
    expect(personaIndex(rows).size).toBe(0)
  })

  it("lets an agent's own id outrank another agent's tier spelling", () => {
    const impostor: PersonaRow = { agent: 'assistant-operations-opus', config: { main: { endpoint: 'spark', model: 'qwen3-14b' } } }
    // Row order must not decide this, so assert it both ways round.
    expect(personaTargets('assistant-operations-opus', [penny, impostor])).toEqual([{ endpoint: 'spark', model: 'qwen3-14b' }])
    expect(personaTargets('assistant-operations-opus', [impostor, penny])).toEqual([{ endpoint: 'spark', model: 'qwen3-14b' }])
  })
})

// ── The agent-configured effort default ──────────────────────────────────────
//
// The same config walk as `personaIndex`, reading one more field: the default
// reasoning effort an admin set beside the model in the agent editor. Same
// defensive posture (a malformed field is no default), same two-pass claim
// order (an agent's own id outranks another's tier spelling).

describe('the configured effort default', () => {
  const row: PersonaRow = {
    agent: 'assistant-operations',
    config: {
      main: { endpoint: 'spark', model: 'qwen3-14b', effort: 'high' },
      aliases: [
        { name: 'opus', endpoint: 'anthropic', model: 'claude-opus-4', effort: 'max' },
        { name: 'fast', endpoint: 'spark', model: 'qwen3-4b' },
      ],
    },
  }

  it('answers the base id from main and each tier from its own alias', () => {
    const efforts = personaEffortIndex([row])
    expect(efforts.get('assistant-operations')).toBe('high')
    expect(efforts.get('assistant-operations-opus')).toBe('max')
    // An alias with no configured effort is simply absent — absent IS the
    // "model default" answer, and a null entry would blur it.
    expect(efforts.has('assistant-operations-fast')).toBe(false)
  })

  it('is absent when nothing configured anything', () => {
    expect(personaEffortIndex([penny]).size).toBe(0)
    expect(personaEffortIndex([]).size).toBe(0)
  })

  it('survives the same hostile config shapes the target resolver does', () => {
    const rows: PersonaRow[] = [
      { agent: 'a-one', config: null },
      { agent: 'b-two', config: { main: 'spark/qwen', effort: 'high' } },
      { agent: 'c-three', config: [] },
      // A target that never became a target (no endpoint) carries no effort
      // even though the field is well-formed — same rule as readTarget.
      { agent: 'd-four', config: { main: { model: 'qwen3-14b', effort: 'high' }, aliases: [{ name: 'opus', effort: 'high' }] } },
      // Whitespace-only is no default, matching the trim-then-check read.
      { agent: 'e-five', config: { main: { endpoint: 'spark', model: 'qwen3-14b', effort: '  ' } } },
      // Non-string effort (a hand-edited row) is ignored, not coerced.
      { agent: 'f-six', config: { main: { endpoint: 'spark', model: 'qwen3-14b', effort: 3 } } },
    ]
    const efforts = personaEffortIndex(rows)
    expect(efforts.size).toBe(0)
  })

  it("lets an agent's own id outrank another agent's tier spelling, exactly as the target index does", () => {
    const impostor: PersonaRow = { agent: 'assistant-operations-opus', config: { main: { endpoint: 'spark', model: 'qwen3-14b', effort: 'low' } } }
    // Row order must not decide this, so assert it both ways round.
    expect(personaEffortIndex([row, impostor]).get('assistant-operations-opus')).toBe('low')
    expect(personaEffortIndex([impostor, row]).get('assistant-operations-opus')).toBe('low')
  })
})

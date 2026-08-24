import { describe, expect, it } from 'vitest'
import type { ModelSpec } from '@/server/harness/model'

// SLOT-LEVEL EFFORT, at the one choke point every harness turn passes through.
//
// The Models view's Roles and Platform-agents panels let an admin say "this
// class of work thinks at THIS level" (effort-prefs.ts). These tests hold the
// runner's half of the contract:
//   · a turn whose model came from a slot with a preference carries it;
//   · a caller's own pick always wins (the nearer the ask, the stronger it is);
//   · a slot the admin configured no preference for sends nothing.
//
// Everything else is stubbed; `resolveModel` is injected to name the winning
// chain step, which is the only fact the runner reads to decide the slot.

const PREFS: Record<string, string> = { 'role:utility': 'low', 'agent:muse': 'high' }

/** deps with a capturing transport and a resolvable step, in the shape every
 *  runner test here needs. */
const world = (step: 'pin' | 'role' | 'utility' | 'first-routable') => {
  const requests: Array<{ effort?: string; model: string }> = []
  return {
    requests,
    deps: {
      resolveModel: async () => ({ model: 'm-1', step }),
      slotEffort: async (slot: string) => PREFS[slot] ?? null,
      routing: async (model: string) => ({ endpoints: ['spark'], upstreamModel: model }),
      missingCapabilities: async () => [],
      capabilities: async () => {},
      guardConfig: async () => ({ mode: 'observe' as const, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
      guardText: async () => [],
      recordFindings: async () => {},
      recordRun: async () => {},
      now: () => 0,
      transport: async (req: { effort?: string; model: string }) => {
        requests.push(req)
        return { kind: 'gateway' as const, text: 'ok', toolNames: [], usage: null, contractDropped: false }
      },
    } as never,
  }
}

const { runHarness } = await import('@/server/harness/run')
const { defineHarness } = await import('@/server/harness/define')

const harness = (id: string, spec: Partial<ModelSpec>) =>
  defineHarness<{ q: string }, string>({
    id,
    label: id,
    job: 'test',
    model: spec,
    requires: [],
    floor: { capabilities: [], refuseBelow: false, note: 'test' },
    render: (input) => [{ role: 'user', content: input.q }],
    output: { kind: 'text', clean: (raw) => raw.trim() || null },
    onFailure: 'null',
  })

describe('slot effort on a harness turn', () => {
  it('carries the admin slot preference when the caller picked nothing', async () => {
    const r = world('pin')
    await runHarness(harness('effort:pin', { pin: 'muse' }), { q: 'draft' }, { caller: 't', deps: r.deps })
    expect(r.requests[0]).toMatchObject({ effort: 'high' })
  })

  it('applies the role slot when the role step won', async () => {
    const r = world('role')
    await runHarness(harness('effort:role', { role: 'utility' }), { q: 'chore' }, { caller: 't', deps: r.deps })
    expect(r.requests[0]).toMatchObject({ effort: 'low' })
  })

  it('treats the utility step as the utility role slot', async () => {
    const r = world('utility')
    await runHarness(harness('effort:utility', {}), { q: 'chore' }, { caller: 't', deps: r.deps })
    expect(r.requests[0]).toMatchObject({ effort: 'low' })
  })

  it('the caller’s own pick beats the slot preference', async () => {
    const r = world('pin')
    await runHarness(
      harness('effort:caller', { pin: 'muse' }),
      { q: 'draft' },
      { caller: 't', effort: 'medium', deps: r.deps },
    )
    expect(r.requests[0]).toMatchObject({ effort: 'medium' })
  })

  it('sends no effort for a slot without a preference (or no slot at all)', async () => {
    const unpinned = world('pin') // step pin, but the harness names no pin → no slot
    await runHarness(harness('effort:nopin', {}), { q: 'x' }, { caller: 't', deps: unpinned.deps })
    expect(unpinned.requests[0]).not.toHaveProperty('effort')

    const auto = world('first-routable')
    await runHarness(harness('effort:auto', {}), { q: 'x' }, { caller: 't', deps: auto.deps })
    expect(auto.requests[0]).not.toHaveProperty('effort')
  })
})

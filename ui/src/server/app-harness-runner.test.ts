import { describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import { appHarnessRunner, runHarnessForApp, type AppHarnessBinding, type RunnerEdges } from './app-harness-runner'
import { appInScope, withApp } from './app-scope'
import { defineHarness, type HarnessDefinition } from './harness/define'
import type { HarnessResult, RunContext } from './harness/run'
import type { GatewayModel } from './llm-gateway'

// THE BOUND RUNNER'S PROMISE, case by case: the host — not the app — decides
// who a run is, where its spend goes, which agent it may drive and which model
// it may pin. Every edge is injected, so nothing here reaches a database, the
// fleet or a model; `run` is a spy that records what the real runner would
// have been handed.

const TRIAGE = z.object({ severity: z.enum(['low', 'high']) })

const triage = defineHarness<{ text: string }, z.infer<typeof TRIAGE>>({
  id: 'helpdesk:triage',
  label: 'Triage',
  job: 'Grades a ticket.',
  requires: ['json'],
  floor: { capabilities: [], refuseBelow: false, note: 'Any model that can return JSON.' },
  model: { role: 'utility' },
  render: (i) => [{ role: 'user', content: i.text }],
  output: { kind: 'json', schema: TRIAGE },
  onFailure: 'null',
})

const ok = <O>(model: string | null): HarnessResult<O> => ({
  value: null,
  model,
  step: null,
  widened: false,
  repairs: 0,
  schemaValid: true,
  answered: true,
  refused: false,
  findings: [],
  raw: null,
  latencyMs: 1,
  escalate: false,
})

const catalog: GatewayModel[] = [
  { id: 'cheap', endpoints: ['spark'], qualified: false },
  { id: 'opus-big', endpoints: ['spark'], qualified: false },
]

interface Spy {
  calls: Array<{ def: HarnessDefinition<unknown, unknown>; ctx: RunContext }>
  warnings: string[]
}

/** The fleet: `helper` and `pa-alice` are agents; everything else is a
 *  gateway model. */
function edges(spy: Spy, over: Partial<RunnerEdges> = {}): Partial<RunnerEdges> {
  return {
    fleetAgentOf: async (m) => (m === 'helper' || m === 'pa-alice' ? m : m === 'helper-opus' ? 'helper' : null),
    userRole: async () => 'member',
    allowlist: async () => ['cheap'],
    catalog: async () => catalog,
    run: (async (def: HarnessDefinition<unknown, unknown>, _input: unknown, ctx: RunContext) => {
      spy.calls.push({ def, ctx })
      return ok(ctx.model ?? 'resolved')
    }) as RunnerEdges['run'],
    runStreamed: (async (def: HarnessDefinition<unknown, unknown>, _input: unknown, ctx: RunContext) => {
      spy.calls.push({ def, ctx })
      return ok(ctx.model ?? 'resolved')
    }) as RunnerEdges['runStreamed'],
    warn: (m) => spy.warnings.push(m),
    ...over,
  }
}

const spyOf = (): Spy => ({ calls: [], warnings: [] })

const requestRunner = (spy: Spy, usable: string[] = ['helper'], extra: Partial<AppHarnessBinding> = {}) =>
  appHarnessRunner({
    app: 'helpdesk',
    actor: { kind: 'request', user: { id: 'u-1', role: 'member' }, usableAgents: async () => usable },
    edges: edges(spy),
    ...extra,
  })

describe('every run is the app’s, whatever the app says', () => {
  it('stamps the caller, books the spend to the app and binds the slot', async () => {
    const spy = spyOf()
    await requestRunner(spy).run(triage, { text: 'checkout is down' })
    const { def, ctx } = spy.calls[0]!
    expect(ctx.caller).toBe('app:helpdesk:helpdesk:triage')
    // No source rides along: the relay books every call on the app's key as `app`.
    expect(ctx.ledger).toEqual({})
    expect(ctx.userId).toBe('u-1')
    expect(def.model).toEqual({ role: 'utility', slot: 'app:helpdesk/helpdesk:triage' })
    // No deps reach the runner.
    expect(ctx.deps).toBeUndefined()
  })

  it("runs as the app it was built for, whatever app scope the call arrived in", async () => {
    const seen: Array<string | null> = []
    const scoped = (async (_def: unknown, _input: unknown, ctx: RunContext) => {
      seen.push(appInScope())
      return ok(ctx.model ?? 'resolved')
    }) as RunnerEdges['run']
    const runner = appHarnessRunner({
      app: 'helpdesk',
      actor: { kind: 'trigger' },
      edges: { ...edges(spyOf()), run: scoped, runStreamed: scoped as unknown as RunnerEdges['runStreamed'] },
    })
    // Outside every scope (a trigger), and inside another app's: the relay
    // must still be handed this app's key.
    await runner.run(triage, { text: 'x' })
    await withApp('billing', () => runner.stream(triage, { text: 'x' }, { onDelta: () => {} }))
    expect(seen).toEqual(['helpdesk', 'helpdesk'])
  })

  it('overwrites a slot the app named itself', async () => {
    const spy = spyOf()
    const sneaky = { ...triage, model: { ...triage.model, slot: 'app:other/their-harness' } }
    await requestRunner(spy).run(sneaky, { text: 'x' })
    expect(spy.calls[0]!.def.model.slot).toBe('app:helpdesk/helpdesk:triage')
  })

  it('keeps UUID ledger ids and drops anything else, with a warning', async () => {
    const spy = spyOf()
    const task = '9b2f6a0e-3c1d-4e5f-8a7b-0c1d2e3f4a5b'
    await requestRunner(spy).run(triage, { text: 'x' }, { ledger: { refId: 'order-42', taskId: task } })
    expect(spy.calls[0]!.ctx.ledger).toEqual({ taskId: task })
    expect(spy.warnings.join('\n')).toMatch(/ledger\.refId "order-42" is not a UUID/)
  })

  it('ignores deps smuggled in through the options', async () => {
    const spy = spyOf()
    const transport = vi.fn()
    await requestRunner(spy).run(triage, { text: 'x' }, { deps: { transport } } as never)
    expect(spy.calls[0]!.ctx.deps).toBeUndefined()
  })

  it('applies the JSON floor to a definition built without defineHarness', async () => {
    const spy = spyOf()
    const raw = { ...triage, floor: { capabilities: [], refuseBelow: false, note: 'n' } } as HarnessDefinition<{ text: string }, { severity: 'low' | 'high' }>
    await requestRunner(spy).run(raw, { text: 'x' })
    expect(spy.calls[0]!.def.floor).toEqual({ capabilities: ['json'], refuseBelow: true, note: 'n' })
  })

  it('refuses a definition that breaks the platform rules, without a model call', async () => {
    const spy = spyOf()
    const broken = { ...triage, guard: { rules: ['not_a_rule'] } }
    const r = await requestRunner(spy).run(broken, { text: 'x' })
    expect(spy.calls).toHaveLength(0)
    expect(r.value).toBeNull()
    expect(r.answered).toBe(false)
    expect(r.error).toMatch(/guard rule "not_a_rule" does not exist/)
  })

  it("honors onFailure: 'throw' for a refusal", async () => {
    const spy = spyOf()
    const throwing = { ...triage, onFailure: 'throw' as const }
    await expect(requestRunner(spy, []).run(throwing, { text: 'x' }, { model: 'helper' })).rejects.toThrow(/may not use that agent/)
  })
})

describe('an agent pin, from a request', () => {
  it('runs on an agent the signed-in person may use', async () => {
    const spy = spyOf()
    const r = await requestRunner(spy, ['helper']).run(triage, { text: 'x' }, { model: 'helper' })
    expect(r.error).toBeUndefined()
    expect(spy.calls[0]!.ctx.model).toBe('helper')
  })

  it('checks a tier id by the agent it belongs to', async () => {
    const spy = spyOf()
    await requestRunner(spy, ['helper']).run(triage, { text: 'x' }, { model: 'helper-opus' })
    expect(spy.calls).toHaveLength(1)
  })

  it("refuses someone else's personal assistant", async () => {
    const spy = spyOf()
    const r = await requestRunner(spy, ['helper']).run(triage, { text: 'x' }, { model: 'pa-alice' })
    expect(spy.calls).toHaveLength(0)
    expect(r.error).toBe('app "helpdesk" may not run harness "helpdesk:triage" on agent "pa-alice": the signed-in person may not use that agent')
  })

  it('refuses when the agents the person may use cannot be read', async () => {
    const spy = spyOf()
    const runner = appHarnessRunner({
      app: 'helpdesk',
      actor: { kind: 'request', user: { id: 'u-1', role: 'member' }, usableAgents: async () => Promise.reject(new Error('503')) },
      edges: edges(spy),
    })
    const r = await runner.run(triage, { text: 'x' }, { model: 'helper' })
    expect(spy.calls).toHaveLength(0)
    expect(r.error).toMatch(/could not be read/)
  })
})

describe('a gateway model pin is held to the member model allowlist', () => {
  it('allows a model the allowlist opens', async () => {
    const spy = spyOf()
    await requestRunner(spy).run(triage, { text: 'x' }, { model: 'cheap' })
    expect(spy.calls).toHaveLength(1)
  })

  it('refuses one it does not', async () => {
    const spy = spyOf()
    const r = await requestRunner(spy).run(triage, { text: 'x' }, { model: 'opus-big' })
    expect(spy.calls).toHaveLength(0)
    expect(r.error).toMatch(/"opus-big": that model is not open to the person this run acts for/)
  })

  it('lets an admin pin anything', async () => {
    const spy = spyOf()
    const runner = appHarnessRunner({
      app: 'helpdesk',
      actor: { kind: 'request', user: { id: 'u-admin', role: 'admin' }, usableAgents: async () => [] },
      edges: edges(spy, { userRole: async () => 'admin' }),
    })
    await runner.run(triage, { text: 'x' }, { model: 'opus-big' })
    expect(spy.calls).toHaveLength(1)
  })
})

describe('from an MCP tool, only the calling agent', () => {
  const mcpRunner = (spy: Spy, caller: { name: string; id: string | null; proven: boolean }, actingUserId: string | null = null) =>
    appHarnessRunner({ app: 'helpdesk', actor: { kind: 'mcp', caller, actingUserId }, edges: edges(spy) })

  it('runs on the agent that called the tool', async () => {
    const spy = spyOf()
    await mcpRunner(spy, { name: 'helper', id: 'a-1', proven: true }).run(triage, { text: 'x' }, { model: 'helper' })
    expect(spy.calls).toHaveLength(1)
    expect(spy.calls[0]!.ctx.userId).toBeUndefined()
  })

  it('refuses any other agent', async () => {
    const spy = spyOf()
    const r = await mcpRunner(spy, { name: 'helper', id: 'a-1', proven: true }).run(triage, { text: 'x' }, { model: 'pa-alice' })
    expect(spy.calls).toHaveLength(0)
    expect(r.error).toMatch(/only on the agent calling the tool \("helper"\)/)
  })

  it('refuses even itself when the caller was not proven', async () => {
    const spy = spyOf()
    const r = await mcpRunner(spy, { name: 'helper', id: null, proven: false }).run(triage, { text: 'x' }, { model: 'helper' })
    expect(spy.calls).toHaveLength(0)
    expect(r.error).toMatch(/was not proven by its own key/)
  })

  it('acts for the person a personal assistant acts for', async () => {
    const spy = spyOf()
    const r = await mcpRunner(spy, { name: 'pa-alice', id: 'a-2', proven: true }, 'u-alice').run(triage, { text: 'x' }, { model: 'opus-big' })
    expect(r.error).toMatch(/not open to the person/)
    await mcpRunner(spy, { name: 'pa-alice', id: 'a-2', proven: true }, 'u-alice').run(triage, { text: 'x' })
    expect(spy.calls[0]!.ctx.userId).toBe('u-alice')
  })
})

describe('a triggered run is the app, with no user', () => {
  it('drives no agent', async () => {
    const spy = spyOf()
    const runner = appHarnessRunner({ app: 'helpdesk', actor: { kind: 'trigger' }, edges: edges(spy) })
    const r = await runner.run(triage, { text: 'x' }, { model: 'helper' })
    expect(spy.calls).toHaveLength(0)
    expect(r.error).toMatch(/a triggered run acts as the app/)
    await runner.run(triage, { text: 'x' }, { model: 'opus-big' })
    expect(spy.calls).toHaveLength(1)
    expect(spy.calls[0]!.ctx.userId).toBeUndefined()
  })
})

describe('streaming', () => {
  it('runs the same bound context through the streamed runner', async () => {
    const spy = spyOf()
    await requestRunner(spy).stream(triage, { text: 'x' }, { onDelta: () => {} })
    expect(spy.calls[0]!.ctx.caller).toBe('app:helpdesk:helpdesk:triage')
  })

  it('follows the context signal and the caller’s own', async () => {
    const spy = spyOf()
    const outer = new AbortController()
    const own = new AbortController()
    await requestRunner(spy, [], { signal: outer.signal }).run(triage, { text: 'x' }, { signal: own.signal })
    const signal = spy.calls[0]!.ctx.signal!
    expect(signal.aborted).toBe(false)
    own.abort()
    expect(signal.aborted).toBe(true)
  })
})

describe('the free runHarness', () => {
  it('drops deps and warns', async () => {
    const warnings: string[] = []
    const seen: RunContext[] = []
    await runHarnessForApp(triage, { text: 'x' }, { caller: 'app:x', deps: { transport: vi.fn() } } as never, {
      run: (async (_d: unknown, _i: unknown, ctx: RunContext) => {
        seen.push(ctx)
        return ok(null)
      }) as RunnerEdges['run'],
      warn: (m) => warnings.push(m),
      fleetAgentOf: async () => null,
    })
    expect(seen[0]!.deps).toBeUndefined()
    expect(warnings.join('\n')).toMatch(/deps from app code; they are ignored/)
  })

  it('warns when it pins an agent, and still runs', async () => {
    const warnings: string[] = []
    const seen: RunContext[] = []
    await runHarnessForApp(triage, { text: 'x' }, { caller: 'app:x', model: 'pa-alice' }, {
      run: (async (_d: unknown, _i: unknown, ctx: RunContext) => {
        seen.push(ctx)
        return ok(null)
      }) as RunnerEdges['run'],
      warn: (m) => warnings.push(m),
      fleetAgentOf: async (m) => (m === 'pa-alice' ? m : null),
    })
    expect(seen[0]!.model).toBe('pa-alice')
    expect(warnings.join('\n')).toMatch(/pins agent "pa-alice" with no check .* ctx\.harness\.run/)
  })
})

describe('where a loop tool goes, per context', () => {
  it("from a request: the app's own tools as this run, and no org tools, saying why", async () => {
    const spy = spyOf()
    const ownTool = vi.fn(async () => ({ text: 'ok', isError: false }))
    const runner = appHarnessRunner({
      app: 'helpdesk',
      actor: { kind: 'request', user: { id: 'u-1', role: 'member' }, usableAgents: async () => [] },
      edges: { ...edges(spy), ownTool },
    })
    await runner.run(triage, { text: 'x' })
    const loop = spy.calls[0]!.ctx.loop!
    expect(loop).toMatchObject({ app: 'helpdesk', actingUserId: 'u-1' })
    expect(loop.org).toBeUndefined()
    expect(loop.orgUnavailable).toMatch(/started by a person, not an agent/)
    const signal = new AbortController().signal
    await loop.mcp!('orders_recent', { a: 1 }, signal)
    expect(ownTool).toHaveBeenCalledWith('helpdesk', 'orders_recent', { a: 1 }, {
      caller: { name: 'app:helpdesk:helpdesk:triage', id: null, proven: true },
      actingUserId: 'u-1',
      signal,
    })
  })

  it('from an MCP tool: as the calling agent, with the org caller the gateway route lent', async () => {
    const spy = spyOf()
    const ownTool = vi.fn(async () => ({ text: 'ok', isError: false }))
    const orgTools = vi.fn(async () => ({ text: 'docs', isError: false }))
    const caller = { name: 'helper', id: 'a-1', proven: true }
    const runner = appHarnessRunner({ app: 'helpdesk', actor: { kind: 'mcp', caller, actingUserId: null }, orgTools, edges: { ...edges(spy), ownTool } })
    await runner.run(triage, { text: 'x' })
    const loop = spy.calls[0]!.ctx.loop!
    expect(loop.org).toBe(orgTools)
    await loop.mcp!('t', {}, new AbortController().signal)
    expect((ownTool.mock.calls[0] as unknown[])[3]).toMatchObject({ caller })
  })

  it('from a trigger: no org tools, saying why', async () => {
    const spy = spyOf()
    await appHarnessRunner({ app: 'helpdesk', actor: { kind: 'trigger' }, edges: edges(spy) }).run(triage, { text: 'x' })
    expect(spy.calls[0]!.ctx.loop!.orgUnavailable).toMatch(/a triggered run has none/)
  })

  it("keeps loop edges out of the free runner's reach", async () => {
    const seen: RunContext[] = []
    await runHarnessForApp(triage, { text: 'x' }, { caller: 'app:x', loop: { app: 'x', actingUserId: null, simulate: true } } as never, {
      run: (async (_d: unknown, _i: unknown, ctx: RunContext) => {
        seen.push(ctx)
        return ok(null)
      }) as RunnerEdges['run'],
      warn: () => {},
      fleetAgentOf: async () => null,
    })
    expect(seen[0]!.loop).toBeUndefined()
  })
})

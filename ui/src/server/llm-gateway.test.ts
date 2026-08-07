import { beforeEach, describe, expect, it, vi } from 'vitest'

// The gateway's parameter learner, exercised end to end on the fetch path —
// classification and TTL are proven purely in harness/gateway-params.test.ts,
// and what is left to prove is the WIRING, which is exactly where audit 1.2
// lived: a contract parameter got stripped, the call succeeded, and nothing
// anywhere said so.
//
// Everything below the gateway is stubbed (no database, no network, no provider
// keys). The settings store is an in-memory map so the persist/reload cycle is
// real rather than mocked away.
const { store, capabilityWrites, routableEndpoints, guardCalls } = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  capabilityWrites: [] as Array<{ key: string; cap: string; value: boolean; source: string }>,
  // Empty by default so `buildUpstream`-level cases keep routing through the
  // explicit `route` they pass; the `completeViaGateway` cases push the endpoint
  // in, because that helper resolves its own route.
  routableEndpoints: [] as unknown[],
  guardCalls: [] as string[],
}))

vi.mock('./db/pg', () => ({ db: () => Promise.reject(new Error('no database in this test')) }))
vi.mock('./audit', () => ({
  getSetting: <T>(key: string, fallback: T): Promise<T> =>
    Promise.resolve(store.has(key) ? (store.get(key) as T) : fallback),
  setSetting: (key: string, value: unknown): Promise<void> => {
    store.set(key, structuredClone(value))
    return Promise.resolve()
  },
}))
vi.mock('./agent-defs', () => ({ listEndpoints: () => Promise.resolve(routableEndpoints) }))
vi.mock('./provider-catalog', () => ({
  NATIVE_BASE: {} as Record<string, string>,
  openrouterUsPool: () => Promise.resolve(null),
  resolveEndpointKey: () => Promise.resolve('sk-test'),
}))
vi.mock('./guardrails', () => ({
  guardCompletion: (arg: { answer: string }): Promise<void> => {
    guardCalls.push(arg.answer)
    return Promise.resolve()
  },
}))
vi.mock('./harness/capability', async (orig) => {
  const real = await orig<typeof import('./harness/capability')>()
  return {
    ...real,
    recordCapability: (key: string, cap: string, fact: { value: boolean; source: string }): Promise<void> => {
      capabilityWrites.push({ key, cap, value: fact.value, source: fact.source })
      return Promise.resolve()
    },
  }
})

import type { LlmEndpoint } from './agent-defs'
import { buildUpstream, completeViaGateway, contractDropsOf, fetchUpstream, forgetLearnedParams, type ResolvedRoute } from './llm-gateway'

const endpoint: LlmEndpoint = {
  id: 'ep1',
  name: 'pl-main',
  provider: 'openai-compatible',
  baseUrl: 'http://spark-a:8000/v1',
  class: 'local',
  apiKeyEnv: null,
  hasKey: true,
  contextLength: null,
  priceInPerMtok: null,
  priceOutPerMtok: null,
  models: ['qwen3-14b'],
  modelPrices: {},
  autoPrices: {},
  requestDefaults: {},
}
const route: ResolvedRoute = { endpoint, upstreamModel: 'qwen3-14b' }
const KEY = 'pl-main:qwen3-14b'
const SETTINGS_KEY = 'gateway_unsupported_params'

/** A fetch that 400s on the named parameter until it stops being sent. */
const rejecting = (param: string): ReturnType<typeof vi.fn> =>
  vi.fn((_url: string, init: { body: string }) => {
    const body = JSON.parse(init.body) as Record<string, unknown>
    if (param in body) {
      return Promise.resolve(new Response(`{"error":{"message":"\`${param}\` is not supported"}}`, { status: 400 }))
    }
    return Promise.resolve(new Response('{"choices":[{"message":{"content":"hi"}}]}', { status: 200 }))
  })

beforeEach(async () => {
  await forgetLearnedParams()
  store.clear()
  capabilityWrites.length = 0
  routableEndpoints.length = 0
  guardCalls.length = 0
  vi.restoreAllMocks()
})

describe('the parameter learner', () => {
  it('strips a cosmetic parameter silently and remembers it, exactly as before', async () => {
    const fetchMock = rejecting('temperature')
    vi.stubGlobal('fetch', fetchMock)

    const call = await buildUpstream(route, { model: 'qwen3-14b', messages: [], temperature: 0.7 })
    const res = await fetchUpstream(call, route)

    expect(res.status).toBe(200)
    expect(fetchMock).toHaveBeenCalledTimes(2)
    expect(contractDropsOf(call)).toEqual([]) // a tunable is nobody's contract
    expect(capabilityWrites).toEqual([]) // and says nothing about capability
    expect(store.get(SETTINGS_KEY)).toHaveProperty([KEY, 'temperature'])

    // Remembered: the next call never sends it, and never pays the 400.
    const next = await buildUpstream(route, { model: 'qwen3-14b', messages: [], temperature: 0.7 })
    expect(next.body.temperature).toBeUndefined()
  })

  it('reports a dropped response_format instead of quietly returning prose', async () => {
    // THE bug. Before: the parameter vanished, the retry succeeded, and the
    // caller fed free prose to a JSON parser with no idea anything had changed.
    const fetchMock = rejecting('response_format')
    vi.stubGlobal('fetch', fetchMock)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})

    const call = await buildUpstream(route, {
      model: 'qwen3-14b',
      messages: [],
      response_format: { type: 'json_object' },
    })
    const res = await fetchUpstream(call, route)

    // The CALL still succeeds — a completed request the caller knows is
    // unconstrained beats a 400 it can do nothing with.
    expect(res.status).toBe(200)
    expect(contractDropsOf(call)).toEqual([
      { param: 'response_format', capability: 'json', endpoint: 'pl-main', model: 'qwen3-14b', source: 'rejected' },
    ])
    // Recorded where the role picker and the model self-test will read it.
    expect(capabilityWrites).toEqual([{ key: KEY, cap: 'json', value: false, source: 'learned' }])
    // And said out loud, naming the model and the parameter.
    expect(warn).toHaveBeenCalledTimes(1)
    expect(warn.mock.calls[0]?.[0]).toContain('response_format')
    expect(warn.mock.calls[0]?.[0]).toContain('qwen3-14b')
  })

  it('keeps reporting the drop on later calls, when nothing 400s at all', async () => {
    // The pre-strip path is the ONLY one that removes the parameter for the next
    // thirty days. A signal that fired once, on the call that learned it, would
    // leave every later caller in exactly the dark this fix exists to end.
    vi.stubGlobal('fetch', rejecting('response_format'))
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const first = await buildUpstream(route, { model: 'qwen3-14b', messages: [], response_format: { type: 'json_object' } })
    await fetchUpstream(first, route)

    const later = await buildUpstream(route, { model: 'qwen3-14b', messages: [], response_format: { type: 'json_object' } })
    expect(later.body.response_format).toBeUndefined()
    expect(contractDropsOf(later)).toEqual([
      { param: 'response_format', capability: 'json', endpoint: 'pl-main', model: 'qwen3-14b', source: 'remembered' },
    ])
    // Re-recording from a remembered strip would restamp the fact on every
    // process start and its TTL would never fire.
    expect(capabilityWrites).toHaveLength(1)
  })

  it('reports nothing when the caller never sent the parameter', async () => {
    vi.stubGlobal('fetch', rejecting('response_format'))
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const first = await buildUpstream(route, { model: 'qwen3-14b', messages: [], response_format: { type: 'json_object' } })
    await fetchUpstream(first, route)

    const plain = await buildUpstream(route, { model: 'qwen3-14b', messages: [] })
    expect(contractDropsOf(plain)).toEqual([])
  })

  it('relays a 400 about a protected parameter rather than making the response unreadable', async () => {
    // Dropping `stream` yields a valid single JSON body that an SSE pump waits
    // on forever. An honest error is the better outcome.
    const fetchMock = rejecting('stream')
    vi.stubGlobal('fetch', fetchMock)

    const call = await buildUpstream(route, { model: 'qwen3-14b', messages: [], stream: true })
    const res = await fetchUpstream(call, route)

    expect(res.status).toBe(400)
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(call.body.stream).toBe(true)
    expect(store.get(SETTINGS_KEY)).toBeUndefined()
  })

  it('forgetLearnedParams clears the strip so the next call asks again', async () => {
    vi.stubGlobal('fetch', rejecting('temperature'))
    const call = await buildUpstream(route, { model: 'qwen3-14b', messages: [], temperature: 0.7 })
    await fetchUpstream(call, route)
    expect(store.get(SETTINGS_KEY)).toHaveProperty([KEY, 'temperature'])

    await forgetLearnedParams(KEY)

    expect(store.get(SETTINGS_KEY)).toEqual({})
    const after = await buildUpstream(route, { model: 'qwen3-14b', messages: [], temperature: 0.7 })
    expect(after.body.temperature).toBe(0.7)
  })

  it('adopts a legacy un-timestamped store and writes it back stamped', async () => {
    // Every existing install has Record<string, string[]> in this row. It must
    // keep working on the very next call, and it must become expirable.
    //
    // The settings row is read ONCE per process (the load promise is memoized —
    // this path runs before every upstream call and must not re-query), so a
    // fresh module instance is the only honest way to test a cold start.
    store.set(SETTINGS_KEY, { [KEY]: ['temperature'] })
    vi.stubGlobal('fetch', rejecting('nothing_at_all'))
    vi.resetModules()
    const cold = await import('./llm-gateway')

    const call = await cold.buildUpstream(route, { model: 'qwen3-14b', messages: [], temperature: 0.7 })
    expect(call.body.temperature).toBeUndefined() // still honored

    const stored = store.get(SETTINGS_KEY) as Record<string, Record<string, string>>
    expect(typeof stored[KEY]?.temperature).toBe('string')
    expect(Date.parse(stored[KEY]?.temperature ?? '')).toBeGreaterThan(0)
  })
})

// The two slots `runHarness` needs from this helper. Their ABSENCE was two
// separate findings: with no `responseFormat`, `inbox-focus-assistant` grew a
// second, weaker request helper so that the same command was strict-JSON or
// prompt-and-pray depending on which model the user picked (audit 1.3); with no
// `guard`, a runner that does its own guard pass files a second guard_findings
// row for one reply and inflates the per-model confabulation rate that the
// fitness page reads.
describe('completeViaGateway structured output', () => {
  const ok = (): ReturnType<typeof vi.fn> => vi.fn(() => Promise.resolve(new Response('{"choices":[{"message":{"content":"{\\"a\\":1}"}}]}', { status: 200 })))

  it('sends response_format when asked, and nothing when not', async () => {
    routableEndpoints.push(endpoint)
    const fetchMock = ok()
    vi.stubGlobal('fetch', fetchMock)

    await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't', responseFormat: 'json_object' })
    const sent = JSON.parse((fetchMock.mock.calls[0]?.[1] as { body: string }).body) as Record<string, unknown>
    expect(sent.response_format).toEqual({ type: 'json_object' })

    await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't' })
    const plain = JSON.parse((fetchMock.mock.calls[1]?.[1] as { body: string }).body) as Record<string, unknown>
    expect(plain.response_format).toBeUndefined()
  })

  it('reports a rejected response_format as a json contract drop instead of returning prose in silence', async () => {
    // Audit 1.2, end to end and on the helper the harness runner actually calls:
    // the retry succeeds, so a 200 proves nothing about the SHAPE of the reply.
    routableEndpoints.push(endpoint)
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, init: { body: string }) => {
        const body = JSON.parse(init.body) as Record<string, unknown>
        if ('response_format' in body) return Promise.resolve(new Response('{"error":{"message":"`response_format` is not supported"}}', { status: 400 }))
        return Promise.resolve(new Response('{"choices":[{"message":{"content":"Sure! Here is the answer."}}]}', { status: 200 }))
      }),
    )

    const res = await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't', responseFormat: 'json_object' })

    expect(res.text).toBe('Sure! Here is the answer.')
    expect(res.contractDrops).toEqual([{ param: 'response_format', capability: 'json', endpoint: 'pl-main', model: 'qwen3-14b', source: 'rejected' }])
    expect(capabilityWrites).toEqual([{ key: KEY, cap: 'json', value: false, source: 'learned' }])
  })

  it('still reports the drop on later calls, when the pre-strip is doing the dropping', async () => {
    // The learning path is where this could rot: after the first 400, only
    // `buildUpstream` removes the parameter, and a caller that trusted the
    // remembered path would go back to handing prose to a JSON parser for the
    // next thirty days.
    routableEndpoints.push(endpoint)
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, init: { body: string }) => {
        const body = JSON.parse(init.body) as Record<string, unknown>
        if ('response_format' in body) return Promise.resolve(new Response('{"error":{"message":"`response_format` is not supported"}}', { status: 400 }))
        return Promise.resolve(new Response('{"choices":[{"message":{"content":"prose"}}]}', { status: 200 }))
      }),
    )
    await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't', responseFormat: 'json_object' })

    const later = await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't', responseFormat: 'json_object' })
    expect(later.contractDrops).toEqual([{ param: 'response_format', capability: 'json', endpoint: 'pl-main', model: 'qwen3-14b', source: 'remembered' }])
  })

  it('guards by default and steps aside for a caller that guards itself', async () => {
    routableEndpoints.push(endpoint)
    vi.stubGlobal('fetch', ok())

    await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't' })
    expect(guardCalls).toHaveLength(1)

    await completeViaGateway('qwen3-14b', [{ role: 'user', content: 'hi' }], { caller: 't', guard: false })
    expect(guardCalls).toHaveLength(1)
  })
})

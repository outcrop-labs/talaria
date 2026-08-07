import { beforeEach, describe, expect, it, vi } from 'vitest'

// The settings store is stubbed with an in-memory map, so these cases exercise
// the real read/merge/expiry logic with no database anywhere near them. The
// mock clones on write for the same reason `setSetting` serializes to JSON: a
// caller must never end up holding a live reference into storage.
const { store } = vi.hoisted(() => ({ store: new Map<string, unknown>() }))

vi.mock('../audit', () => ({
  getSetting: <T>(key: string, fallback: T): Promise<T> =>
    Promise.resolve(store.has(key) ? (store.get(key) as T) : fallback),
  setSetting: (key: string, value: unknown): Promise<void> => {
    store.set(key, structuredClone(value))
    return Promise.resolve()
  },
}))

import {
  capabilityKey,
  forgetCapabilities,
  getCapabilities,
  mergeCapabilities,
  missingCapabilities,
  outranks,
  recordCapability,
  LEARNED_TTL_MS,
  type Capability,
  type CapabilityFact,
} from './capability'
import { LEARNED_PARAM_TTL_MS } from './gateway-params'

const SETTINGS_KEY = 'model_capabilities'
const KEY = capabilityKey('pl-main', 'qwen3-14b')
const OTHER = capabilityKey('openrouter', 'qwen3-14b')

const iso = (msAgo: number): string => new Date(Date.now() - msAgo).toISOString()

const fact = (over: Partial<CapabilityFact> = {}): CapabilityFact => ({
  value: true,
  source: 'probe',
  at: iso(0),
  ...over,
})

/** Write straight into the stubbed row, bypassing `recordCapability`, so a read
 *  case can stage shapes the writer would never produce (stale, malformed). */
const seed = (rows: Record<string, Record<string, unknown>>): void => {
  store.set(SETTINGS_KEY, structuredClone(rows))
}

const raw = (): Record<string, Record<string, unknown>> =>
  (store.get(SETTINGS_KEY) as Record<string, Record<string, unknown>>) ?? {}

beforeEach(() => {
  store.clear()
})

// ── capabilityKey ───────────────────────────────────────────────────────────

describe('capabilityKey', () => {
  it('spells the key exactly as llm-gateway does for its learned params', () => {
    // `${route.endpoint.name}:${route.upstreamModel}` — the two stores must
    // agree, because an admin clearing one expects to be clearing both.
    expect(capabilityKey('pl-main', 'qwen3-14b')).toBe('pl-main:qwen3-14b')
  })

  it('separates the same model id served by two different endpoints', () => {
    expect(capabilityKey('pl-main', 'qwen3-14b')).not.toBe(capabilityKey('openrouter', 'qwen3-14b'))
  })
})

// ── getCapabilities ─────────────────────────────────────────────────────────

describe('getCapabilities', () => {
  it('returns nothing for a model nobody has recorded anything about', async () => {
    expect(await getCapabilities(KEY)).toEqual({})
  })

  it('round-trips detail and score', async () => {
    const f = fact({ value: true, source: 'probe', detail: '5/5 nested objects parsed', score: 1 })
    await recordCapability(KEY, 'json-strict', f)
    expect((await getCapabilities(KEY))['json-strict']).toEqual(f)
  })

  it('drops a learned fact past the TTL, so the provider can be re-discovered', async () => {
    seed({ [KEY]: { json: fact({ value: false, source: 'learned', at: iso(LEARNED_TTL_MS + 60_000) }) } })
    expect(await getCapabilities(KEY)).toEqual({})
  })

  it('keeps a learned fact still inside the TTL', async () => {
    const f = fact({ value: false, source: 'learned', at: iso(LEARNED_TTL_MS - 60_000) })
    seed({ [KEY]: { json: f } })
    expect((await getCapabilities(KEY))?.json).toEqual(f)
  })

  it('keeps a probe fact of any age — a measurement expires when someone re-measures', async () => {
    const f = fact({ value: false, source: 'probe', at: iso(LEARNED_TTL_MS * 12), score: 0.1 })
    seed({ [KEY]: { json: f } })
    expect((await getCapabilities(KEY))?.json).toEqual(f)
  })

  it('keeps a declared fact of any age — a human said so and no clock overrides that', async () => {
    const f = fact({ value: true, source: 'declared', at: iso(LEARNED_TTL_MS * 12) })
    seed({ [KEY]: { search: f } })
    expect((await getCapabilities(KEY))?.search).toEqual(f)
  })

  it('treats a learned fact with an unparseable timestamp as expired, not eternal', async () => {
    seed({ [KEY]: { json: { value: false, source: 'learned', at: 'sometime last tuesday' } } })
    expect(await getCapabilities(KEY)).toEqual({})
  })

  it('ignores malformed entries instead of crashing a harness run', async () => {
    seed({
      [KEY]: {
        json: { value: 'yes', source: 'probe', at: iso(0) }, // value is not a boolean
        tools: { value: true, source: 'vibes', at: iso(0) }, // source is not a source
        search: { value: true, source: 'probe' }, // no timestamp
        vision: null,
        code: fact({ value: true }), // the one good row
      },
    })
    expect(Object.keys(await getCapabilities(KEY))).toEqual(['code'])
  })

  it('ignores capability ids this build does not recognize', async () => {
    seed({ [KEY]: { 'audio-in': fact(), json: fact() } })
    expect(Object.keys(await getCapabilities(KEY))).toEqual(['json'])
  })
})

// ── recordCapability ────────────────────────────────────────────────────────

describe('recordCapability', () => {
  it('merges: other capabilities on the same key survive', async () => {
    await recordCapability(KEY, 'json', fact({ value: true }))
    await recordCapability(KEY, 'search', fact({ value: false, detail: 'no citations in 5/5 trials' }))

    const facts = await getCapabilities(KEY)
    expect(facts.json?.value).toBe(true)
    expect(facts.search?.value).toBe(false)
    expect(facts.search?.detail).toBe('no citations in 5/5 trials')
  })

  it('leaves other endpoint:model records untouched', async () => {
    await recordCapability(OTHER, 'search', fact({ value: true }))
    await recordCapability(KEY, 'search', fact({ value: false }))

    expect((await getCapabilities(OTHER)).search?.value).toBe(true)
    expect((await getCapabilities(KEY)).search?.value).toBe(false)
  })

  it('lets a probe correct what the gateway learned from a single 400', async () => {
    await recordCapability(KEY, 'json', fact({ value: false, source: 'learned', detail: 'upstream rejected response_format' }))
    await recordCapability(KEY, 'json', fact({ value: true, source: 'probe', score: 1, detail: '10/10 json_object calls honored' }))

    const json = (await getCapabilities(KEY)).json
    expect(json?.value).toBe(true)
    expect(json?.source).toBe('probe')
    expect(json?.score).toBe(1)
  })

  it('sweeps expired facts out of the row it rewrites', async () => {
    seed({ [KEY]: { search: fact({ value: false, source: 'learned', at: iso(LEARNED_TTL_MS * 2) }) } })
    await recordCapability(KEY, 'json', fact())
    expect(Object.keys(raw()[KEY] ?? {})).toEqual(['json'])
  })

  it('preserves capability ids it does not recognize, so a rolling deploy loses nothing', async () => {
    seed({ [KEY]: { 'audio-in': fact({ detail: 'written by a newer build' }) } })
    await recordCapability(KEY, 'json', fact())
    expect(Object.keys(raw()[KEY] ?? {}).sort()).toEqual(['audio-in', 'json'])
  })

  it('does not lose writes when a probe run records a whole battery at once', async () => {
    const battery: Capability[] = ['json', 'json-strict', 'tools', 'tool-select', 'search']
    await Promise.all(battery.map((cap) => recordCapability(KEY, cap, fact({ value: true }))))
    expect(Object.keys(await getCapabilities(KEY)).sort()).toEqual([...battery].sort())
  })
})

// ── missingCapabilities ─────────────────────────────────────────────────────

describe('missingCapabilities', () => {
  it('reports nothing missing for a model nobody has tested — UNKNOWN IS NOT FALSE', async () => {
    // The single most important behavior in this module. A fresh self-host has
    // probed nothing; if an unknown capability read as "no", every harness with
    // a `requires` list would refuse to run on install day.
    expect(await missingCapabilities(KEY, ['json', 'tools', 'search', 'vision'])).toEqual([])
  })

  it('reports only the capabilities positively recorded as false', async () => {
    await recordCapability(KEY, 'json', fact({ value: true }))
    await recordCapability(KEY, 'search', fact({ value: false }))
    // 'tools' is never recorded: unknown, therefore not missing.
    expect(await missingCapabilities(KEY, ['json', 'search', 'tools'])).toEqual(['search'])
  })

  it('does not report a capability the caller did not ask about', async () => {
    await recordCapability(KEY, 'vision', fact({ value: false }))
    expect(await missingCapabilities(KEY, ['json'])).toEqual([])
  })

  it('lets an expired learned "no" go back to unknown rather than staying missing', async () => {
    // This is the ratchet release: the gateway learned 30 days ago that this
    // endpoint rejected response_format. The provider may well have fixed it,
    // and the model gets to try again instead of being written off forever.
    seed({ [KEY]: { json: fact({ value: false, source: 'learned', at: iso(LEARNED_TTL_MS + 1000) }) } })
    expect(await missingCapabilities(KEY, ['json'])).toEqual([])
  })

  it('keeps reporting a probe "no" no matter how old it is', async () => {
    seed({ [KEY]: { search: fact({ value: false, source: 'probe', at: iso(LEARNED_TTL_MS * 6) }) } })
    expect(await missingCapabilities(KEY, ['search'])).toEqual(['search'])
  })

  it('returns empty for a harness that requires nothing', async () => {
    await recordCapability(KEY, 'json', fact({ value: false }))
    expect(await missingCapabilities(KEY, [])).toEqual([])
  })
})

// ── forgetCapabilities ──────────────────────────────────────────────────────

describe('forgetCapabilities', () => {
  it('clears every fact for one model, probe and declared included', async () => {
    await recordCapability(KEY, 'json', fact({ value: true, source: 'probe' }))
    await recordCapability(KEY, 'search', fact({ value: true, source: 'declared' }))
    await forgetCapabilities(KEY)
    expect(await getCapabilities(KEY)).toEqual({})
  })

  it('forgets one endpoint:model without touching its neighbors', async () => {
    await recordCapability(KEY, 'json', fact())
    await recordCapability(OTHER, 'json', fact())
    await forgetCapabilities(KEY)
    expect(Object.keys(await getCapabilities(OTHER))).toEqual(['json'])
  })

  it('is a no-op on a model with no record, and does not create an empty one', async () => {
    await forgetCapabilities(KEY)
    expect(raw()).toEqual({})
  })
})

// ── The tie between the two learned-fact stores ──────────────────────────────

describe('the learned TTL', () => {
  it('is the same number the gateway uses to expire a learned parameter strip', () => {
    // Two stores record ONE event from two angles: `gateway_unsupported_params`
    // remembers what we stopped sending; `model_capabilities` remembers what
    // that told us about the model. They are separate constants because
    // `gateway-params.ts` is pure by construction and importing this module
    // would put a database on the path of every upstream call — so the
    // agreement is held here, by a test, instead of by a comment in each file.
    //
    // If the strip outlived its capability fact, the gateway would go on quietly
    // dropping `response_format` while Admin reported the model as
    // unknown-but-fine. That is audit 1.2 growing back with the fix in place.
    expect(LEARNED_TTL_MS).toBe(LEARNED_PARAM_TTL_MS)
  })
})


// ── Source precedence ────────────────────────────────────────────────────────

describe('mergeCapabilities', () => {
  it('ranks probe over declared over catalog over learned', () => {
    expect(outranks('probe', 'declared')).toBe(true)
    expect(outranks('declared', 'catalog')).toBe(true)
    expect(outranks('catalog', 'learned')).toBe(true)
    expect(outranks('catalog', 'probe')).toBe(false)
    expect(outranks('catalog', 'declared')).toBe(false)
    expect(outranks('learned', 'catalog')).toBe(false)
    // Equal rank is last-write-wins, so re-probing can correct a probe.
    expect(outranks('probe', 'probe')).toBe(true)
    // Nothing there means anything may write.
    expect(outranks('learned', undefined)).toBe(true)
  })

  it('does not let a nightly catalog refresh erase a probe result', async () => {
    // THE REGRESSION THIS EXISTS TO PREVENT. An admin probes a model, finds its
    // tool calling broken, and the provider's catalog cheerfully advertises
    // `tools`. Under last-write-wins the finding evaporates the next night.
    await recordCapability(KEY, 'tools', fact({ value: false, source: 'probe', detail: 'called no tool on 4 of 4 prompts' }))

    const written = await mergeCapabilities(KEY, { tools: fact({ value: true, source: 'catalog' }) })

    expect(written).toBe(0)
    const after = await getCapabilities(KEY)
    expect(after.tools?.value).toBe(false)
    expect(after.tools?.source).toBe('probe')
  })

  it('does let a catalog fill in what nothing has measured, and a probe correct it later', async () => {
    expect(await mergeCapabilities(KEY, { json: fact({ source: 'catalog' }) })).toBe(1)
    expect((await getCapabilities(KEY)).json).toMatchObject({ value: true, source: 'catalog' })

    await mergeCapabilities(KEY, { json: fact({ value: false, source: 'probe', detail: 'returned prose on 3 of 3' }) })
    expect((await getCapabilities(KEY)).json).toMatchObject({ value: false, source: 'probe' })
  })

  it('never lets a catalog overrule the human who typed it', async () => {
    await recordCapability(KEY, 'vision', fact({ value: false, source: 'declared', detail: 'our deployment strips images' }))
    await mergeCapabilities(KEY, { vision: fact({ value: true, source: 'catalog' }) })
    expect((await getCapabilities(KEY)).vision).toMatchObject({ value: false, source: 'declared' })
  })

  it('replaces an EXPIRED incumbent whatever it claimed to be', async () => {
    seed({ [KEY]: { tools: { value: false, source: 'learned', at: iso(LEARNED_TTL_MS + 60_000) } } })
    expect(await mergeCapabilities(KEY, { tools: fact({ source: 'catalog' }) })).toBe(1)
    expect((await getCapabilities(KEY)).tools).toMatchObject({ value: true, source: 'catalog' })
  })

  it('writes many keys in one pass and leaves other keys alone', async () => {
    await recordCapability(OTHER, 'json', fact())
    const written = await mergeCapabilities([
      { key: KEY, facts: { tools: fact({ source: 'catalog' }), json: fact({ source: 'catalog' }) } },
      { key: 'spark:llama', facts: { tools: fact({ source: 'catalog' }) } },
    ])
    expect(written).toBe(3)
    expect(Object.keys(await getCapabilities(KEY)).sort()).toEqual(['json', 'tools'])
    expect((await getCapabilities(OTHER)).json?.source).toBe('probe')
  })

  it('preserves capability ids this build does not recognize', async () => {
    // A rolling deploy can put a newer build alongside this one; rewriting the
    // row must not delete facts it wrote under a capability we have not heard of.
    seed({ [KEY]: { 'audio-input': { value: true, source: 'probe', at: iso(0) } } })
    await mergeCapabilities(KEY, { tools: fact({ source: 'catalog' }) })
    expect(raw()[KEY]?.['audio-input']).toBeDefined()
  })

  it('is a no-op on an empty batch rather than churning the row', async () => {
    expect(await mergeCapabilities([])).toBe(0)
  })

  it('expires a catalog fact on the same clock as a learned one', async () => {
    // Both are re-derivable without asking anyone, so both decay if the thing
    // that produced them goes away.
    seed({ [KEY]: { tools: { value: true, source: 'catalog', at: iso(LEARNED_TTL_MS + 60_000) } } })
    expect((await getCapabilities(KEY)).tools).toBeUndefined()
  })
})

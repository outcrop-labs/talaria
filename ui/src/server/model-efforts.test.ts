import { beforeEach, describe, expect, it } from 'vitest'
import { effortsForModel, ensureEffortsCatalog, resetEffortsBackfill, type EffortDeps } from '@/server/model-efforts'
import type { CatalogStore } from '@/server/model-catalog'
import type { LlmEndpoint, ModelTarget } from '@/server/agent-defs'

// THE COMPOSER'S QUESTION, answered once: "may THIS id be asked for an effort,
// and at which levels?" The two spellings of a model id — a fleet persona and
// a gateway catalog id — resolve through different halves of the same stored
// metadata, and everything here is injected: no database, no settings row.

const model = (id: string, efforts: string[] | null) => ({
  id,
  name: null,
  contextLength: null,
  inputModalities: null,
  supportedParameters: null,
  efforts,
  pricing: null,
})

const bench = (
  store: CatalogStore,
  personas: Record<string, ModelTarget[]> = {},
  personasThrow = false,
): Partial<EffortDeps> => ({
  read: async () => store,
  personaTargets: async (model: string) => {
    if (personasThrow) throw new Error('connection terminated unexpectedly')
    return personas[model] ?? []
  },
})

describe('effortsForModel', () => {
  const store: CatalogStore = {
    spark: { endpoint: 'spark', at: '2026-08-10T00:00:00.000Z', models: [model('qwen3-14b', ['low', 'medium', 'high'])] },
    selfhost: { endpoint: 'selfhost', at: '2026-08-10T00:00:00.000Z', models: [model('qwen3-14b', null)] },
  }

  it('resolves a persona id through its targets, never as a catalog id', async () => {
    // "dex-ops" is in no catalog; it is a pointer at spark:qwen3-14b. Reading
    // the catalog branch for it would answer [] and hide the picker on the
    // one surface (agent DMs) where it matters most.
    const deps = bench(store, { 'dex-ops': [{ endpoint: 'spark', model: 'qwen3-14b' }] })
    expect(await effortsForModel('dex-ops', deps)).toEqual(['low', 'medium', 'high'])
  })

  it('intersects the persona pool — a fallback that publishes fewer levels wins', async () => {
    // The pool is main plus fallbacks because EITHER may take the call, so a
    // level is offered only when both accept it.
    const deps = bench(store, {
      'dex-ops': [
        { endpoint: 'spark', model: 'qwen3-14b' },
        { endpoint: 'selfhost', model: 'qwen3-14b' },
      ],
    })
    // selfhost publishes nothing, so it does not veto; a fallback that DID
    // publish a narrower list would. Assert the non-veto half here…
    expect(await effortsForModel('dex-ops', deps)).toEqual(['low', 'medium', 'high'])
    // …and the veto half with a publishing-but-narrow fallback.
    const narrow: CatalogStore = { ...store, narrow: { endpoint: 'narrow', at: store.spark!.at, models: [model('qwen3-14b', ['high'])] } }
    const depsNarrow = bench(narrow, {
      'dex-ops': [
        { endpoint: 'spark', model: 'qwen3-14b' },
        { endpoint: 'narrow', model: 'qwen3-14b' },
      ],
    })
    expect(await effortsForModel('dex-ops', depsNarrow)).toEqual(['high'])
  })

  it('falls back to the catalog spelling when the id is not a persona', async () => {
    const deps = bench(store)
    expect(await effortsForModel('qwen3-14b', deps)).toEqual(['low', 'medium', 'high'])
    expect(await effortsForModel('spark/qwen3-14b', deps)).toEqual(['low', 'medium', 'high'])
  })

  it('answers [] for a model with no levels published', async () => {
    const deps = bench(store)
    expect(await effortsForModel('selfhost/qwen3-14b', deps)).toEqual([])
  })

  it('never throws — a broken edge is the no-picker answer', async () => {
    // A chat turn must not fail because a picker could not decide whether to
    // appear; the persona index and the settings read both degrade to [].
    expect(await effortsForModel('dex-ops', bench(store, {}, true))).toEqual([])
    const broken: Partial<EffortDeps> = {
      read: async () => {
        throw new Error('connection terminated unexpectedly')
      },
      personaTargets: async () => [],
    }
    expect(await effortsForModel('qwen3-14b', broken)).toEqual([])
  })
})

// ── The backfill ─────────────────────────────────────────────────────────────
//
// THE STALE-CATALOG CASE THIS EXISTS FOR: the only production writer of the
// stored catalog is the model-adder modal, so a deployment upgraded past the
// effort extraction still holds catalogs written before it — models present,
// no `efforts` key on any of them. Those must be refreshed once; a catalog
// written by the current build (efforts present, even null) must never be.

describe('ensureEffortsCatalog', () => {
  // The throttle is module state shared by the whole file's cases; each test
  // starts from a clean window so one case's attempt never silences another's.
  beforeEach(resetEffortsBackfill)

  /** A store whose spark entry predates the effort extraction. The cast is
   *  the point: the row is exactly what the OLD writer stored, which is not a
   *  shape the current build's type describes. */
  const preEfforts = (): CatalogStore =>
    ({
      spark: {
        endpoint: 'spark',
        at: '2026-08-01T00:00:00.000Z',
        models: [
          { id: 'qwen3-14b', name: null, contextLength: null, inputModalities: null, supportedParameters: ['tools'], pricing: null },
        ],
      },
    }) as unknown as CatalogStore
  const endpoint = (name: string): LlmEndpoint =>
    ({ id: `id-${name}`, name, provider: 'openrouter', baseUrl: null, class: 'cloud', apiKeyEnv: null, hasKey: true, contextLength: null }) as LlmEndpoint

  it('refreshes a pre-efforts catalog once and answers with the fresh levels', async () => {
    const store = preEfforts()
    const refreshed: string[] = []
    const deps: Partial<EffortDeps> = {
      ...bench(store, { 'dex-ops': [{ endpoint: 'spark', model: 'qwen3-14b' }] }),
      endpoints: async () => [endpoint('spark')],
      refreshEndpoint: async (ep) => {
        refreshed.push(ep.name)
        // The refresh writes the CURRENT build's shape: every model carries
        // the efforts key (a list, or null).
        store.spark = {
          endpoint: 'spark',
          at: '2026-08-20T00:00:00.000Z',
          models: [{ ...store.spark!.models[0]!, efforts: ['low', 'high'] }],
        }
      },
    }

    expect(await ensureEffortsCatalog('dex-ops', deps)).toEqual(['low', 'high'])
    expect(refreshed).toEqual(['spark'])
    // The refreshed entry no longer reads as pre-feature — a second ask is a
    // pure settings read.
    expect(await ensureEffortsCatalog('dex-ops', deps)).toEqual(['low', 'high'])
    expect(refreshed).toEqual(['spark'])
  })

  it('does not fetch for a catalog the current build wrote — null is an answer', async () => {
    const store: CatalogStore = {
      spark: { endpoint: 'spark', at: '2026-08-20T00:00:00.000Z', models: [model('qwen3-14b', null)] },
    }
    const deps: Partial<EffortDeps> = {
      ...bench(store),
      endpoints: async () => [endpoint('spark')],
      refreshEndpoint: async () => {
        throw new Error('must not be called')
      },
    }
    expect(await ensureEffortsCatalog('qwen3-14b', deps)).toEqual([])
  })

  it('does not fetch for a model no catalog carries', async () => {
    const deps: Partial<EffortDeps> = {
      ...bench({}),
      endpoints: async () => {
        throw new Error('must not be called')
      },
      refreshEndpoint: async () => {
        throw new Error('must not be called')
      },
    }
    expect(await ensureEffortsCatalog('nothing-serves-this', deps)).toEqual([])
  })

  it('a failed refresh is the stored answer, and the throttle holds the retry', async () => {
    const store = preEfforts()
    let attempts = 0
    const deps: Partial<EffortDeps> = {
      ...bench(store, { 'dex-ops': [{ endpoint: 'spark', model: 'qwen3-14b' }] }),
      endpoints: async () => [endpoint('spark')],
      refreshEndpoint: async () => {
        attempts++
        throw new Error('provider answered 503')
      },
    }
    expect(await ensureEffortsCatalog('dex-ops', deps)).toEqual([])
    expect(attempts).toBe(1)
    // Within the retry window the throttle answers from the store without
    // another live call — a provider that is down must not turn every picker
    // question into a timeout.
    expect(await ensureEffortsCatalog('dex-ops', deps)).toEqual([])
    expect(attempts).toBe(1)
  })
})

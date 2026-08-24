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
  roster: LlmEndpoint[] = [],
): Partial<EffortDeps> => ({
  read: async () => store,
  personaTargets: async (model: string) => {
    if (personasThrow) throw new Error('connection terminated unexpectedly')
    return personas[model] ?? []
  },
  endpoints: async () => roster,
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

// ── Admin-declared ladders: the second voice ─────────────────────────────────
//
// THE SELF-HOST CASE THIS COLUMN EXISTS FOR: a minimal OpenAI-compatible
// server answers /models with `{id}` and nothing else — no parameters, no
// ladder — so the provider's voice is permanently silent no matter what the
// weights accept. The endpoint's own operator is the one person who can know,
// and `llm_endpoints.model_efforts` is their say. A declaration REPLACES the
// catalog's ladder for that endpoint's build of the model (never merges — a
// union would offer a level neither voice vouched for) and speaks for its own
// pool member only, so the persona pool's intersection still holds.
describe('effortsForModel · declared ladders', () => {
  const at = '2026-08-20T00:00:00.000Z'
  const store: CatalogStore = {
    spark: { endpoint: 'spark', at, models: [model('qwen3-14b', ['low', 'medium', 'high'])] },
    selfhost: { endpoint: 'selfhost', at, models: [model('qwen3-14b', null)] },
  }
  const declaring = (name: string, efforts: Record<string, unknown>): LlmEndpoint =>
    ({ id: `id-${name}`, name, provider: 'custom', baseUrl: null, class: 'local', apiKeyEnv: null, hasKey: true, contextLength: null, modelEfforts: efforts }) as unknown as LlmEndpoint

  it('fills a silent catalog — the self-host whose operator knows', async () => {
    const roster = [declaring('selfhost', { 'qwen3-14b': ['low', 'medium', 'high', 'xhigh'] })]
    expect(await effortsForModel('selfhost/qwen3-14b', bench(store, {}, false, roster))).toEqual(['low', 'medium', 'high', 'xhigh'])
  })

  it('replaces a published ladder — a declaration never merges with the catalog', async () => {
    // The operator says this build only takes low and high; offering medium
    // because the catalog says so would send a level the model may reject.
    const roster = [declaring('spark', { 'qwen3-14b': ['low', 'high'] })]
    expect(await effortsForModel('spark/qwen3-14b', bench(store, {}, false, roster))).toEqual(['low', 'high'])
    // Scoped to the endpoint it was declared on: the same upstream model on a
    // DIFFERENT endpoint keeps that endpoint's own word (here: the catalog's).
    expect(await effortsForModel('spark/qwen3-14b', bench(store, {}, false, [declaring('selfhost', { 'qwen3-14b': ['low'] })]))).toEqual(['low', 'medium', 'high'])
  })

  it('speaks for its own pool member only — the persona pool still intersects', async () => {
    // A persona over spark (catalog: low/medium/high) with a declared
    // selfhost fallback (declared: low/high): the pool may land on either, so
    // only what both accept is offered.
    const roster = [declaring('selfhost', { 'qwen3-14b': ['low', 'high'] })]
    const deps = bench(store, {
      'dex-ops': [
        { endpoint: 'spark', model: 'qwen3-14b' },
        { endpoint: 'selfhost', model: 'qwen3-14b' },
      ],
    }, false, roster)
    expect(await effortsForModel('dex-ops', deps)).toEqual(['low', 'high'])
  })

  it('a member with no declaration and no catalog still does not veto', async () => {
    // The declaration on one member must not turn the OTHERS into vetoes:
    // members that say nothing are skipped, exactly as before the column.
    const roster = [declaring('selfhost', { 'qwen3-14b': ['low', 'high'] })]
    const deps = bench(
      { ...store, third: { endpoint: 'third', at, models: [model('qwen3-14b', null)] } },
      {
        'dex-ops': [
          { endpoint: 'selfhost', model: 'qwen3-14b' },
          { endpoint: 'third', model: 'qwen3-14b' },
        ],
      },
      false,
      roster,
    )
    expect(await effortsForModel('dex-ops', deps)).toEqual(['low', 'high'])
  })

  it('ignores a malformed declaration — admin-typed JSON degrades to the catalog', async () => {
    // The column is JSON an admin typed that outlives the build that wrote
    // it; a malformed entry must read as silence, not crash a chat turn's
    // validation.
    const roster = [declaring('selfhost', { 'qwen3-14b': ['ok', 42] })]
    expect(await effortsForModel('selfhost/qwen3-14b', bench(store, {}, false, roster))).toEqual([])
    const emptyRoster = [declaring('selfhost', { 'qwen3-14b': [] })]
    expect(await effortsForModel('selfhost/qwen3-14b', bench(store, {}, false, emptyRoster))).toEqual([])
  })

  it('a broken roster read is the catalog answer — never a thrown picker question', async () => {
    const broken: Partial<EffortDeps> = {
      ...bench(store),
      endpoints: async () => {
        throw new Error('connection terminated unexpectedly')
      },
    }
    expect(await effortsForModel('spark/qwen3-14b', broken)).toEqual(['low', 'medium', 'high'])
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

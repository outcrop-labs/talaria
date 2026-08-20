import { describe, expect, it } from 'vitest'
import { advertisedWindow, capabilitiesFromCatalog, catalogEntriesFor, catalogEntriesForTargets, effortLevelsOf, effortsFor, refreshCatalogs, refreshEndpointCatalog, type CatalogDeps, type CatalogStore } from '@/server/model-catalog'
import type { CatalogModel } from '@/server/provider-catalog'
import type { Capability, CapabilityFact } from '@/server/harness/capability'
import type { LlmEndpoint } from '@/server/agent-defs'

// THE CATALOG LAYER, and the property under test throughout is the yes-only
// rule: a provider may prove a capability and may never disprove one. Every
// assertion about an ABSENT field checks that nothing was written, not that
// `false` was written — see the header of model-catalog.ts for why the
// difference decides whether a run gets refused.

const model = (over: Partial<CatalogModel> = {}): CatalogModel => ({
  id: 'm',
  name: null,
  contextLength: null,
  inputModalities: null,
  supportedParameters: null,
  efforts: null,
  pricing: null,
  ...over,
})

const endpoint = (name: string): LlmEndpoint =>
  ({ id: `id-${name}`, name, provider: 'openrouter', baseUrl: null, class: 'cloud', apiKeyEnv: null, hasKey: true, contextLength: null }) as LlmEndpoint

const AT = '2026-08-07T00:00:00.000Z'

interface Bench {
  deps: Partial<CatalogDeps>
  store: CatalogStore
  written: Array<{ key: string; facts: Partial<Record<Capability, CapabilityFact>> }>
}

function bench(over: Partial<CatalogDeps> = {}, seed: CatalogStore = {}): Bench {
  const store: CatalogStore = { ...seed }
  const written: Bench['written'] = []
  return {
    store,
    written,
    deps: {
      read: async () => store,
      write: async (next) => {
        for (const k of Object.keys(store)) delete store[k]
        Object.assign(store, next)
      },
      record: async (batch) => {
        written.push(...batch)
        return batch.reduce((n, e) => n + Object.keys(e.facts).length, 0)
      },
      now: () => Date.parse(AT),
      ...over,
    },
  }
}

describe('capabilitiesFromCatalog', () => {
  it('declares what the provider advertises', () => {
    const facts = capabilitiesFromCatalog(
      model({ supportedParameters: ['tools', 'tool_choice', 'response_format', 'structured_outputs'], inputModalities: ['text', 'image'] }),
      AT,
    )
    expect(facts.tools).toMatchObject({ value: true, source: 'catalog' })
    expect(facts.json).toMatchObject({ value: true, source: 'catalog' })
    expect(facts['json-strict']).toMatchObject({ value: true, source: 'catalog' })
    expect(facts.vision).toMatchObject({ value: true, source: 'catalog' })
    // The detail is what an admin reads next to the fact, so it says where the
    // claim came from rather than restating the capability's name.
    expect(facts.tools?.detail).toContain('provider catalog')
  })

  it('NEVER writes false — an absent field is unknown, not a denial', () => {
    // A self-hosted OpenAI-compatible server answering /models with ids and
    // nothing else. Under a trust-both-directions rule this model would be
    // recorded as unable to call tools, and `run.ts` step 2 would then REFUSE
    // the judge on it.
    const bare = capabilitiesFromCatalog(model(), AT)
    expect(bare).toEqual({})

    // And a catalog that DOES describe parameters but omits one still says
    // nothing about the one it omitted.
    const partial = capabilitiesFromCatalog(model({ supportedParameters: ['temperature', 'top_p'] }), AT)
    expect(partial).toEqual({})
    expect(Object.values(capabilitiesFromCatalog(model({ inputModalities: ['text'] }), AT))).toEqual([])
  })

  it('reads native web search off the one parameter that proves it', () => {
    expect(capabilitiesFromCatalog(model({ supportedParameters: ['web_search_options'] }), AT).search).toMatchObject({ value: true, source: 'catalog' })
    // A model with tools but no native search gets a `tools` fact and NO
    // `search` fact — which is the whole premise of reaching search by tool.
    const tooler = capabilitiesFromCatalog(model({ supportedParameters: ['tools'] }), AT)
    expect(tooler.tools?.value).toBe(true)
    expect(tooler.search).toBeUndefined()
  })

  it('does not declare long-context from an advertised window', () => {
    // The window says what the model ACCEPTS. Whether it can still find a fact
    // planted in the middle of it is what the probe measures, and a spec sheet
    // must not stand in for that.
    expect(capabilitiesFromCatalog(model({ contextLength: 1_048_576 }), AT)['long-context']).toBeUndefined()
  })

  it('understands the older modality spelling', () => {
    expect(capabilitiesFromCatalog(model({ inputModalities: ['text', 'image'] }), AT).vision?.value).toBe(true)
  })
})

describe('refreshEndpointCatalog', () => {
  it('stores the catalog and merges what it proves in one write', async () => {
    const models = [model({ id: 'a', supportedParameters: ['tools'] }), model({ id: 'b', supportedParameters: ['response_format'] }), model({ id: 'c' })]
    const b = bench({ fetchCatalog: async () => models })

    const out = await refreshEndpointCatalog(endpoint('spark'), b.deps)

    expect(out).toMatchObject({ endpoint: 'spark', models: 3, facts: 2, error: null })
    expect(b.store['spark']?.models).toHaveLength(3)
    // ONE merge for the whole endpoint, and the model that proved nothing is not
    // in it — an empty fact set is not a write.
    expect(b.written.map((e) => e.key)).toEqual(['spark:a', 'spark:b'])
  })

  it('keeps the last good catalog when the provider is unreachable', async () => {
    const seed: CatalogStore = { spark: { endpoint: 'spark', at: '2026-01-01T00:00:00.000Z', models: [model({ id: 'old' })] } }
    const b = bench(
      {
        fetchCatalog: async () => {
          throw new Error('provider answered 503')
        },
      },
      seed,
    )

    const out = await refreshEndpointCatalog(endpoint('spark'), b.deps)

    expect(out.error).toBe('provider answered 503')
    // A blip must not empty the picker or un-declare facts an admin is reading.
    expect(b.store['spark']?.models.map((m) => m.id)).toEqual(['old'])
    expect(b.written).toEqual([])
  })

  it('leaves every other endpoint alone', async () => {
    const seed: CatalogStore = { local: { endpoint: 'local', at: AT, models: [model({ id: 'llama' })] } }
    const b = bench({ fetchCatalog: async () => [model({ id: 'a' })] }, seed)
    await refreshEndpointCatalog(endpoint('spark'), b.deps)
    expect(Object.keys(b.store).sort()).toEqual(['local', 'spark'])
  })
})

describe('refreshCatalogs', () => {
  const eps = [endpoint('spark'), endpoint('local')]

  it('only refetches what has gone stale', async () => {
    const fresh = new Date(Date.parse(AT) - 60_000).toISOString()
    const seed: CatalogStore = { spark: { endpoint: 'spark', at: fresh, models: [] } }
    const asked: string[] = []
    const b = bench(
      {
        endpoints: async () => eps,
        fetchCatalog: async (ep) => {
          asked.push(ep.name)
          return []
        },
      },
      seed,
    )

    await refreshCatalogs({}, b.deps)
    expect(asked).toEqual(['local'])
  })

  it('refetches everything when forced', async () => {
    const fresh = new Date(Date.parse(AT) - 60_000).toISOString()
    const asked: string[] = []
    const b = bench(
      {
        endpoints: async () => eps,
        fetchCatalog: async (ep) => {
          asked.push(ep.name)
          return []
        },
      },
      { spark: { endpoint: 'spark', at: fresh, models: [] }, local: { endpoint: 'local', at: fresh, models: [] } },
    )

    await refreshCatalogs({ force: true }, b.deps)
    expect(asked).toEqual(['spark', 'local'])
  })
})

describe('advertisedWindow', () => {
  it('takes the SMALLEST window across the endpoints that serve the id', async () => {
    // A bare id can land on any member of the pool, so a claim has to hold for
    // the worst of them.
    const b = bench(
      {},
      {
        spark: { endpoint: 'spark', at: AT, models: [model({ id: 'qwen', contextLength: 128_000 })] },
        local: { endpoint: 'local', at: AT, models: [model({ id: 'qwen', contextLength: 32_768 })] },
      },
    )
    expect(await advertisedWindow('qwen', b.deps)).toBe(32_768)
  })

  it('is null when nothing advertises one, rather than guessing', async () => {
    const b = bench({}, { spark: { endpoint: 'spark', at: AT, models: [model({ id: 'qwen' })] } })
    expect(await advertisedWindow('qwen', b.deps)).toBeNull()
    expect(await advertisedWindow('never-heard-of-it', b.deps)).toBeNull()
  })

  it('reports every endpoint that serves the id, because they can differ', async () => {
    const b = bench(
      {},
      {
        spark: { endpoint: 'spark', at: AT, models: [model({ id: 'qwen', contextLength: 128_000 })] },
        local: { endpoint: 'local', at: AT, models: [model({ id: 'qwen', contextLength: 32_768 })] },
      },
    )
    expect((await catalogEntriesFor('qwen', b.deps)).map((e) => e.endpoint).sort()).toEqual(['local', 'spark'])
  })
})

describe('finding a model in the catalog', () => {
  const store = {
    openrouter: {
      endpoint: 'openrouter',
      at: '2026-08-10T00:00:00.000Z',
      models: [model({ id: 'deepseek/deepseek-v4-flash', contextLength: 1_048_576 })],
    },
    anthropic: { endpoint: 'anthropic', at: '2026-08-10T00:00:00.000Z', models: [model({ id: 'claude-opus-5' })] },
  }
  const deps: Partial<CatalogDeps> = { read: async (): Promise<CatalogStore> => store }

  it('matches the ENDPOINT-QUALIFIED id, which is the only spelling callers use now', async () => {
    // THE REGRESSION THIS LOCKS. A catalog is keyed by the id the provider
    // publishes (`deepseek/deepseek-v4-flash`), and every caller now says
    // `openrouter/deepseek/deepseek-v4-flash` because that is the one spelling
    // the picker offers. Matching the raw id alone answered null for a model
    // advertising a 1,048,576-token window, and the long-context probe duly
    // reported that it advertises none.
    expect(await advertisedWindow('openrouter/deepseek/deepseek-v4-flash', deps)).toBe(1_048_576)
  })

  it('still matches a bare id across every endpoint that serves it', async () => {
    expect(await advertisedWindow('deepseek/deepseek-v4-flash', deps)).toBe(1_048_576)
  })

  it('never lets one endpoint prefix match another endpoint entry', async () => {
    // `anthropic/deepseek/...` is not a thing anthropic serves, and answering
    // from OpenRouter's row would credit one endpoint with another's spec.
    expect(await advertisedWindow('anthropic/deepseek/deepseek-v4-flash', deps)).toBeNull()
  })

  it('answers null for a model no catalog carries', async () => {
    expect(await advertisedWindow('openrouter/nothing-here', deps)).toBeNull()
  })
})

describe('reasoning effort', () => {
  // THE PICKER'S OPTION LIST IS THE CATALOG'S WORD. A level is offered only
  // when the provider published it (`reasoning.supported_efforts`, parsed in
  // provider-catalog.ts), and a pool only offers what EVERY publishing member
  // accepts — the same worst-member rule `advertisedWindow` applies to a
  // window, for the same reason: a bare id can land on any member.

  it('lists the levels every publishing member of the pool accepts', async () => {
    const b = bench(
      {},
      {
        spark: { endpoint: 'spark', at: AT, models: [model({ id: 'qwen', efforts: ['low', 'high'] })] },
        local: { endpoint: 'local', at: AT, models: [model({ id: 'qwen', efforts: ['low', 'medium', 'high'] })] },
      },
    )
    expect(await effortsFor('qwen', b.deps)).toEqual(['low', 'high'])
  })

  it('is empty when nothing vouches for a level — unknown is not false', async () => {
    // A self-host answering /models with ids only, and a model nobody has
    // heard of, are the same answer: no picker, and no effort field on the
    // request. Neither is an error, and neither vetoes a sibling that DOES
    // publish (the test above's `local` had to survive `spark` saying nothing).
    const b = bench({}, { spark: { endpoint: 'spark', at: AT, models: [model({ id: 'qwen', efforts: null })] } })
    expect(await effortsFor('qwen', b.deps)).toEqual([])
    expect(await effortsFor('never-heard-of-it', b.deps)).toEqual([])
  })

  it('answers for the endpoint-qualified spelling', async () => {
    const b = bench(
      {},
      { spark: { endpoint: 'spark', at: AT, models: [model({ id: 'deepseek/v4', efforts: ['low', 'medium', 'high'] })] } },
    )
    expect(await effortsFor('spark/deepseek/v4', b.deps)).toEqual(['low', 'medium', 'high'])
  })

  it('orders the ladder weakest to strongest, provider coinages last', () => {
    // The provider's spelling is the contract — nothing is renamed — but the
    // DISPLAY order is ours, so 'high' never sits under 'low' in a picker.
    expect(effortLevelsOf([{ endpoint: 'e', model: model({ efforts: ['max', 'low', 'turbo'] }) }])).toEqual(['low', 'max', 'turbo'])
  })

  it('serves explicit persona targets by exact endpoint and id', async () => {
    // The persona path does not pool by id: the agent config NAMES the
    // endpoint, so `qwen` on `spark` must never be answered from `other`'s
    // row for the same id.
    const b = bench(
      {},
      {
        spark: { endpoint: 'spark', at: AT, models: [model({ id: 'qwen', efforts: ['low'] })] },
        other: { endpoint: 'other', at: AT, models: [model({ id: 'qwen', efforts: ['high'] })] },
      },
    )
    const entries = await catalogEntriesForTargets([{ endpoint: 'spark', model: 'qwen' }], b.deps)
    expect(entries.map((e) => e.endpoint)).toEqual(['spark'])
    expect(effortLevelsOf(entries)).toEqual(['low'])
  })
})

import { describe, expect, it } from 'vitest'
import { advertisedWindow, catalogEntriesFor, catalogEntriesForTargets, effortLevelsOf, effortsFor, type CatalogDeps, type CatalogModel, type CatalogStore } from '@/server/model-catalog'

// THE CATALOG'S READ HALF — the api fetches and stores it, and proves
// capabilities from it (talaria-model-catalog, whose tests hold the yes-only
// rule); this process only reads what it stored. What is under test here is
// that reading: which entries an id finds, and what a pool agrees on.

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

const AT = '2026-08-07T00:00:00.000Z'

interface Bench {
  deps: Partial<CatalogDeps>
  store: CatalogStore
}

function bench(over: Partial<CatalogDeps> = {}, seed: CatalogStore = {}): Bench {
  const store: CatalogStore = { ...seed }
  return { store, deps: { read: async () => store, ...over } }
}

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

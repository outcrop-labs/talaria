import { beforeEach, describe, expect, it, vi } from 'vitest'

// The catalog and the routing rule — all that is left of the TS gateway. The
// calls themselves go to the Rust relay (harness/transport.ts), and its
// parameter learner, metering and budgets are proven there.
const { routableEndpoints } = vi.hoisted(() => ({ routableEndpoints: [] as unknown[] }))

vi.mock('./agent-defs', () => ({ listEndpoints: () => Promise.resolve(routableEndpoints) }))

import type { LlmEndpoint } from './agent-defs'
import { canonicalModelId, gatewayModels, routingFor, type GatewayModel } from './llm-gateway'

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

beforeEach(() => {
  routableEndpoints.length = 0
})

describe('where a model id can land', () => {
  const ep = (name: string, models: string[]): LlmEndpoint => ({ ...endpoint, id: name, name, models })

  it('pins an endpoint-qualified id to that endpoint and drops the prefix upstream', async () => {
    routableEndpoints.push(ep('anthropic', ['claude-opus-5']), ep('openrouter', ['anthropic/claude-opus-5']))
    const r = await routingFor('anthropic/claude-opus-5')
    expect(r.endpoints.map((e) => e.name)).toEqual(['anthropic'])
    expect(r.upstreamModel).toBe('claude-opus-5')
  })

  it('pools a bare id over every endpoint that serves it', async () => {
    routableEndpoints.push(ep('spark-a', ['qwen3-14b']), ep('spark-b', ['qwen3-14b']))
    const r = await routingFor('qwen3-14b')
    expect(r.endpoints.map((e) => e.name)).toEqual(['spark-a', 'spark-b'])
    expect(r.upstreamModel).toBe('qwen3-14b')
  })
})

describe('the catalog every picker draws from', () => {
  const ep = (name: string, models: string[]): LlmEndpoint => ({ ...endpoint, id: name, name, models })

  it('offers one row per model, spelled endpoint-first', async () => {
    // THE DUPLICATE ROWS. Offering both spellings put `claude-opus-5` and
    // `anthropic/claude-opus-5` on consecutive rows of every picker and of the
    // fitness matrix — one deployment, one capability key, two entries.
    routableEndpoints.push(ep('anthropic', ['claude-opus-5', 'claude-sonnet-5']), ep('openrouter', ['deepseek/deepseek-v4-flash']))

    expect((await gatewayModels()).map((m) => m.id)).toEqual([
      'anthropic/claude-opus-5',
      'anthropic/claude-sonnet-5',
      'openrouter/deepseek/deepseek-v4-flash',
    ])
  })

  it('keeps the bare id when it is a POOL rather than a second name', async () => {
    // Two endpoints serving one model is a round-robin target no qualified id
    // can express, and it is the only case where the bare name means something
    // the endpoint-first one does not.
    routableEndpoints.push(ep('spark-a', ['qwen3-14b']), ep('spark-b', ['qwen3-14b']))
    const models = await gatewayModels()

    expect(models.map((m) => m.id)).toEqual(['qwen3-14b', 'spark-a/qwen3-14b', 'spark-b/qwen3-14b'])
    expect(models.find((m) => m.id === 'qwen3-14b')?.endpoints).toEqual(['spark-a', 'spark-b'])
  })
})

describe('canonicalModelId', () => {
  const catalog: GatewayModel[] = [
    { id: 'anthropic/claude-opus-5', endpoints: ['anthropic'], qualified: true },
    { id: 'spark-a/qwen3-14b', endpoints: ['spark-a'], qualified: true },
    { id: 'spark-b/qwen3-14b', endpoints: ['spark-b'], qualified: true },
    { id: 'qwen3-14b', endpoints: ['spark-a', 'spark-b'], qualified: false },
  ]

  it('maps a stored bare id onto the row now offered for it', () => {
    // Role assignments, agent pins and archived reports were all written when
    // both spellings were on offer. They keep routing; this is what makes them
    // line up with the list an admin is looking at.
    expect(canonicalModelId('claude-opus-5', catalog)).toBe('anthropic/claude-opus-5')
  })

  it('leaves a pooled id alone — picking one endpoint would halve it', () => {
    expect(canonicalModelId('qwen3-14b', catalog)).toBe('qwen3-14b')
  })

  it('leaves an id nothing serves exactly as written', () => {
    // An assignment pointing at a model that has been removed is a fact to
    // show, not one to guess at.
    expect(canonicalModelId('gpt-4o', catalog)).toBe('gpt-4o')
  })

  it('is a no-op on an id already offered', () => {
    expect(canonicalModelId('anthropic/claude-opus-5', catalog)).toBe('anthropic/claude-opus-5')
  })
})

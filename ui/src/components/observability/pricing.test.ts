import { describe, expect, it } from 'vitest'
import {
  REASON_CODES,
  canRetry,
  emptyPriceForm,
  endpointLabel,
  includedBody,
  manualBody,
  parseRate,
  reasonMeta,
  repricedLine,
  settingLabel,
  tokensLine,
} from './pricing'

describe('reasons', () => {
  it('labels every reason the api can send, and a failed source as a failure', () => {
    for (const code of REASON_CODES) expect(reasonMeta(code).label).toBeTruthy()
    expect(reasonMeta('source_failed').tone).toBe('danger')
    expect(reasonMeta('no_match').tone).toBe('warn')
    expect(reasonMeta('not_checked').tone).toBe('neutral')
    // A code this build does not know still renders, as itself.
    expect(reasonMeta('brand_new')).toEqual({ label: 'brand_new', tone: 'neutral' })
  })
})

describe('the row', () => {
  it('names rows booked without an endpoint', () => {
    expect(endpointLabel('')).toBe('(no endpoint)')
    expect(endpointLabel('openrouter')).toBe('openrouter')
  })

  it('offers Retry only where there is a configured endpoint to ask', () => {
    expect(canRetry({ endpoint: 'openai', endpointExists: true })).toBe(true)
    expect(canRetry({ endpoint: 'old-openai', endpointExists: false })).toBe(false)
    expect(canRetry({ endpoint: '', endpointExists: false })).toBe(false)
  })

  it('shows cache tokens only when there are some', () => {
    const m = { promptTokens: 1_200_000, completionTokens: 40_000, cacheWriteTokens: 0, cacheReadTokens: 0 }
    expect(tokensLine(m)).toBe('1.2M in · 40k out')
    expect(tokensLine({ ...m, cacheReadTokens: 900_000 })).toBe('1.2M in · 40k out · 900k cache')
  })
})

describe('the manual price form', () => {
  const target = { endpoint: 'openai', model: 'gpt-4.1-2025-04-14' }

  it('reads blank as not given and refuses what is not a non-negative amount', () => {
    expect(parseRate('')).toEqual({ ok: true, value: null })
    expect(parseRate(' 2.5 ')).toEqual({ ok: true, value: 2.5 })
    expect(parseRate('$0.30')).toEqual({ ok: true, value: 0.3 })
    expect(parseRate('0')).toEqual({ ok: true, value: 0 })
    expect(parseRate('-1')).toEqual({ ok: false })
    expect(parseRate('two')).toEqual({ ok: false })
  })

  it('needs input and output, sends cache rates when given, and null when not', () => {
    const blank = manualBody(target, emptyPriceForm())
    expect(blank).toEqual({ ok: false, error: 'Input and output rates are both required.' })
    const r = manualBody(target, { ...emptyPriceForm(), input: '2', output: '8', cacheRead: '0.5', note: '  OpenAI pricing page ' })
    expect(r).toEqual({
      ok: true,
      body: {
        endpoint: 'openai',
        model: 'gpt-4.1-2025-04-14',
        mode: 'manual',
        priceInPerMtok: 2,
        priceOutPerMtok: 8,
        priceCacheWritePerMtok: null,
        priceCacheReadPerMtok: 0.5,
        note: 'OpenAI pricing page',
      },
    })
  })

  it('names the field that is wrong', () => {
    const r = manualBody(target, { ...emptyPriceForm(), input: '2', output: '8', cacheWrite: 'lots' })
    expect(r).toEqual({ ok: false, error: 'Cache write must be a dollar amount per 1M tokens, 0 or more.' })
  })

  it('marks a model included with no rates at all', () => {
    expect(includedBody({ endpoint: 'zai-plan', model: 'glm-4.6' }, ' Z.ai Coding Plan ')).toEqual({
      endpoint: 'zai-plan',
      model: 'glm-4.6',
      mode: 'included',
      note: 'Z.ai Coding Plan',
    })
    expect(includedBody({ endpoint: '', model: 'm' }, '').note).toBeNull()
  })
})

describe('what a decision says', () => {
  it('never shows an included model as a bare $0', () => {
    expect(settingLabel({ mode: 'included', in: null, out: null, cacheWrite: null, cacheRead: null })).toBe(
      'Included in a plan · $0',
    )
    expect(settingLabel({ mode: 'manual', in: 2, out: 8, cacheWrite: null, cacheRead: 0.5 })).toBe(
      '$2 in · $8 out · $0.5 cache read per 1M',
    )
  })

  it('says how many past calls a save re-priced, and at what', () => {
    expect(repricedLine({ rows: 2, pricedRows: 2, unpricedRows: 0, tokens: 1, cost: 3.9, basis: 'manual' })).toBe(
      'Re-priced 2 past calls: $3.90 (manual).',
    )
    expect(repricedLine({ rows: 1, pricedRows: 1, unpricedRows: 0, tokens: 1, cost: 0, basis: 'included' })).toBe(
      'Re-priced 1 past call: included in a plan, $0.',
    )
    expect(repricedLine({ rows: 3, pricedRows: 0, unpricedRows: 3, tokens: 1, cost: 0, basis: null })).toBe(
      '3 past calls still have no price.',
    )
    expect(repricedLine({ rows: 3, pricedRows: 2, unpricedRows: 1, tokens: 1, cost: 1, basis: null })).toBe(
      'Re-priced 2 past calls: $1.00; 1 still unpriced.',
    )
    expect(repricedLine({ rows: 0, pricedRows: 0, unpricedRows: 0, tokens: 0, cost: 0, basis: null })).toBe(
      'No calls in the last 30 days to re-price.',
    )
  })
})

describe('the spend legend', () => {
  it('labels an included model instead of showing a bare $0', async () => {
    const { modelCostLabel } = await import('@/lib/cost.svelte')
    expect(modelCostLabel({ cost: null, tokens: 10 })).toBe('unpriced')
    expect(modelCostLabel({ cost: 0, tokens: 1000, includedTokens: 1000 })).toBe('included in a plan')
    expect(modelCostLabel({ cost: 1.5, tokens: 3000, includedTokens: 1000 })).toBe('$1.50 + 1.0k included')
    expect(modelCostLabel({ cost: 1.5, tokens: 3000 })).toBe('$1.50')
  })
})

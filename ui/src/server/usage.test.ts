import { describe, expect, it, vi } from 'vitest'

// normalizeUsage is where the ledger starts matching the invoice (#243): the
// two provider shapes in the wild disagree about whether cached input sits
// INSIDE prompt_tokens, and pricing the wrong shape over- or understates the
// bill in a direction nobody notices until reconciliation. These tests pin the
// shape detection and the arithmetic the PRICED SQL transcribes (multipliers
// against the input rate — the SQL itself is exercised by any priced query).
vi.mock('./db/pg', () => ({ db: () => Promise.reject(new Error('no database in this test')) }))
vi.mock('./llm-gateway', () => ({ routingFor: () => Promise.resolve({ endpoints: [], upstreamModel: '' }) }))
vi.mock('./price-oracle', () => ({ nudgeAutoPrices: () => {} }))

const { normalizeUsage, CACHE_READ_MULTIPLIER, CACHE_WRITE_MULTIPLIER } = await import('./usage')

describe('normalizeUsage — provider shape detection', () => {
  it('Anthropic native: cache tokens ride OUTSIDE input_tokens, priced on top', () => {
    const c = normalizeUsage({
      input_tokens: 100,
      cache_creation_input_tokens: 1_000,
      cache_read_input_tokens: 5_000,
      output_tokens: 200,
    })
    expect(c).toEqual({ promptTokens: 100, completionTokens: 200, cacheWriteTokens: 1_000, cacheReadTokens: 5_000, reasoningTokens: 0 })
    // The input side of the bill at any rate R: 100·R + 1000·(1.25R) + 5000·(0.1R)
    // — the flat model priced all 6,100 tokens at 1R and so OVERSTATED here
    // (while UNDERSTATING writes-only traffic). This pins the multiplier
    // arithmetic the PRICED SQL transcribes.
    const inputUnits = c!.promptTokens + c!.cacheWriteTokens * CACHE_WRITE_MULTIPLIER + c!.cacheReadTokens * CACHE_READ_MULTIPLIER
    expect(inputUnits).toBe(100 + 1_250 + 500)
  })

  it('OpenAI-compatible: cached input is INSIDE prompt_tokens — split it out at the read rate', () => {
    const c = normalizeUsage({ prompt_tokens: 6_000, prompt_tokens_details: { cached_tokens: 5_000 }, completion_tokens: 300 })
    expect(c).toEqual({ promptTokens: 1_000, completionTokens: 300, cacheWriteTokens: 0, cacheReadTokens: 5_000, reasoningTokens: 0 })
    // Pricing all 6,000 at 1x would OVERSTATE: reads are a tenth.
    expect(c!.promptTokens + c!.cacheReadTokens).toBe(6_000)
  })

  it('a compat layer reporting detail-object cache WRITES folds them out of prompt_tokens too', () => {
    const c = normalizeUsage({ prompt_tokens: 1_100, prompt_tokens_details: { cache_creation_tokens: 100 } })
    expect(c).toEqual({ promptTokens: 1_000, completionTokens: 0, cacheWriteTokens: 100, cacheReadTokens: 0, reasoningTokens: 0 })
  })

  it('reasoning tokens are recorded for visibility but never re-priced', () => {
    const c = normalizeUsage({ prompt_tokens: 10, completion_tokens: 300, completion_tokens_details: { reasoning_tokens: 200 } })
    expect(c?.reasoningTokens).toBe(200)
    // They are already inside completion_tokens: the four billed fields must
    // not grow when reasoning is reported.
    expect(c!.promptTokens + c!.completionTokens + c!.cacheWriteTokens + c!.cacheReadTokens).toBe(310)
  })

  it('reasoning can never exceed the completion count it claims to live inside', () => {
    const c = normalizeUsage({ prompt_tokens: 10, completion_tokens: 50, completion_tokens_details: { reasoning_tokens: 9_999 } })
    expect(c?.reasoningTokens).toBe(50)
  })

  it('caps cached_tokens at what prompt_tokens actually holds', () => {
    const c = normalizeUsage({ prompt_tokens: 100, prompt_tokens_details: { cached_tokens: 5_000 } })
    expect(c).toEqual({ promptTokens: 0, completionTokens: 0, cacheWriteTokens: 0, cacheReadTokens: 100, reasoningTokens: 0 })
  })

  it('null/absent/garbage usage books nothing — a rejection cannot invent spend', () => {
    expect(normalizeUsage(null)).toBeNull()
    expect(normalizeUsage({})).toBeNull()
    expect(normalizeUsage({ prompt_tokens: Number.NaN, completion_tokens: -5 })).toBeNull()
  })

  it('negative and non-finite counts clamp to zero rather than poisoning the ledger', () => {
    const c = normalizeUsage({ input_tokens: -100, output_tokens: 50 } as never)
    expect(c).toEqual({ promptTokens: 0, completionTokens: 50, cacheWriteTokens: 0, cacheReadTokens: 0, reasoningTokens: 0 })
  })
})

import { beforeEach, describe, expect, it, vi } from 'vitest'

// The spend ceiling (#243): before it, nothing anywhere refused LLM spend — a
// stuck loop, a runaway cron, or a leaked key could spend without limit. These
// tests pin checkBudget's semantics: off by default, org + caller scopes,
// zero/null meaning unlimited, and a failed spend read never becoming an
// outage. spendSince (the SQL read side) is stubbed; everything else about the
// gateway is out of scope here.
const { store, spendBySubject, spendCalls } = vi.hoisted(() => ({
  store: new Map<string, unknown>(),
  // subject ('*' = org) → window; what the stubbed spendSince answers.
  spendBySubject: new Map<string, { tokens: number; cost: number; unpricedTokens: number }>(),
  spendCalls: [] as Array<string | null>,
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
vi.mock('./agent-defs', () => ({ listEndpoints: () => Promise.resolve([]) }))
vi.mock('./provider-catalog', () => ({
  NATIVE_BASE: {},
  openrouterUsPool: () => Promise.resolve(null),
  resolveEndpointKey: () => Promise.resolve('sk-test'),
}))
vi.mock('./guardrails', () => ({ guardCompletion: () => Promise.resolve() }))
vi.mock('./harness/capability', () => ({ capabilityKey: () => '', recordCapability: () => Promise.resolve() }))
vi.mock('./usage', () => ({
  normalizeUsage: () => null,
  spendSince: async (_hours: number, subject?: string | null) => {
    spendCalls.push(subject ?? null)
    const hit = spendBySubject.get(subject ?? '*')
    if (!hit) throw new Error('no window recorded for this subject')
    return hit
  },
}))

const { checkBudget, budgetMessage, setBudgets } = await import('./llm-gateway')

const window = (tokens: number, cost: number) => ({ tokens, cost, unpricedTokens: 0 })

beforeEach(() => {
  store.clear()
  spendBySubject.clear()
  spendCalls.length = 0
})

describe('checkBudget — the circuit breaker', () => {
  it('is OFF by default: unlimited everywhere, and usage_events is never read', async () => {
    await setBudgets({ windowHours: 24, org: null, perAgent: null, agents: {} })
    expect(await checkBudget('api:anyone')).toBeNull()
    expect(spendCalls).toEqual([])
  })

  it('zero limits mean unlimited too — a cleared field can never refuse everything', async () => {
    await setBudgets({ windowHours: 24, org: { tokens: 0, usd: 0 }, perAgent: null, agents: {} })
    expect(await checkBudget('api:anyone')).toBeNull()
    expect(spendCalls).toEqual([])
  })

  it('refuses at the ORG scope once the window reaches the token cap', async () => {
    await setBudgets({ windowHours: 24, org: { tokens: 1000, usd: null }, perAgent: null, agents: {} })
    spendBySubject.set('*', window(1000, 0))
    const d = await checkBudget('api:someone')
    expect(d).toMatchObject({ scope: 'org', subject: null, unit: 'tokens', limit: 1000, used: 1000 })
    expect(budgetMessage(d!)).toContain('organization')
    expect(budgetMessage(d!)).toContain('1,000 tokens')
  })

  it('refuses at the CALLER scope under a per-agent default, without touching the org read', async () => {
    await setBudgets({ windowHours: 24, org: null, perAgent: { tokens: 500, usd: null }, agents: {} })
    spendBySubject.set('api:spendthrift', window(500, 0))
    const d = await checkBudget('api:spendthrift')
    expect(d).toMatchObject({ scope: 'caller', subject: 'api:spendthrift', unit: 'tokens' })
    expect(budgetMessage(d!)).toContain('spendthrift')
  })

  it('a caller with headroom passes — the ceiling only refuses once reached', async () => {
    await setBudgets({ windowHours: 24, org: null, perAgent: { tokens: 500, usd: null }, agents: {} })
    spendBySubject.set('api:frugal', window(499, 0))
    expect(await checkBudget('api:frugal')).toBeNull()
  })

  it('a per-caller OVERRIDE beats the per-agent default in both directions', async () => {
    await setBudgets({ windowHours: 24, org: null, perAgent: { tokens: 500, usd: null }, agents: { 'api:vip': { tokens: 5000, usd: null } } })
    // 4,600 is >80% of the override, so the read is uncached (see below) —
    // and over the 500 default, so passing proves the override won.
    spendBySubject.set('api:vip', window(4_600, 0))
    expect(await checkBudget('api:vip')).toBeNull()
    spendBySubject.set('api:vip', window(5_000, 0))
    const d = await checkBudget('api:vip')
    expect(d).toMatchObject({ scope: 'caller', limit: 5000 })
  })

  it('reads are cached with headroom, and EXACT at the edge — a burst cannot slip 15s of traffic past the check', async () => {
    await setBudgets({ windowHours: 24, org: { tokens: 1000, usd: null }, perAgent: null, agents: {} })
    spendBySubject.set('*', window(400, 0)) // 40% — one read serves both callers
    await checkBudget('api:a')
    await checkBudget('api:b')
    expect(spendCalls.length).toBe(1)
    // setBudgets clears the cache; a fresh read at >80% of the cap gets TTL 0,
    // so every subsequent check re-reads instead of trusting a cached window.
    spendBySubject.set('*', window(850, 0))
    await setBudgets({ windowHours: 24, org: { tokens: 1000, usd: null }, perAgent: null, agents: {} })
    await checkBudget('api:c')
    await checkBudget('api:d')
    expect(spendCalls.length).toBe(3)
  })

  it('a USD ceiling trips on priced spend only — unpriced tokens never move it', async () => {
    await setBudgets({ windowHours: 12, org: null, perAgent: { tokens: null, usd: 10 }, agents: {} })
    spendBySubject.set('api:cloud', { tokens: 9_000_000, cost: 10, unpricedTokens: 9_000_000 })
    const d = await checkBudget('api:cloud')
    expect(d).toMatchObject({ unit: 'usd', limit: 10 })
    expect(budgetMessage(d!)).toContain('$10.00')
  })

  it('the org read failing must not become an outage', async () => {
    await setBudgets({ windowHours: 24, org: { tokens: 1000, usd: null }, perAgent: null, agents: {} })
    // no window recorded → the stubbed spendSince rejects
    expect(await checkBudget('api:anyone')).toBeNull()
  })
})

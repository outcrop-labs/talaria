import { beforeEach, describe, expect, it, vi } from 'vitest'

// The per-key policy (#265), pinned at its write shape: 0 and negatives
// normalize to NULL on the way in (the row and the API must never disagree
// about which spelling means "unlimited"), the update is scoped to the owner
// AND to unrevoked rows, and the hot-path read resolves caps alongside the
// identity — a policy check that cost a second query would tax every call to
// save one setting screen.

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
let updatedRows = 1
let keyRow: Record<string, unknown> | null = null

function answer(text: string): unknown[] {
  if (text.includes('update llm_api_keys set')) return Array.from({ length: updatedRows }, () => ({ id: 'k1' }))
  if (text.includes('from llm_api_keys k')) return keyRow ? [keyRow] : []
  return []
}

const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  return Promise.resolve(answer(text))
}) as unknown as Awaited<ReturnType<typeof import('./db/pg').db>>

vi.mock('./db/pg', () => ({ db: () => Promise.resolve(sql) }))
vi.mock('./permissions', () => ({ hasPerm: () => Promise.resolve(true) }))

const { setKeyPolicy, authenticateKey } = await import('./llm-keys')

beforeEach(() => {
  queries.length = 0
  updatedRows = 1
  keyRow = null
})

describe('setKeyPolicy', () => {
  it('normalizes 0 to null — the row stores one spelling of unlimited', async () => {
    await setKeyPolicy('u1', 'k1', { spendCapTokens: 0, spendCapUsd: 0, rateLimitPerMinute: 0 })
    const q = queries[0]!
    expect(q.values).toEqual([null, null, null, 'k1', 'u1'])
  })

  it('keeps real numbers and refuses nothing else the route already validated', async () => {
    await setKeyPolicy('u1', 'k1', { spendCapTokens: 1000, spendCapUsd: 10.5, rateLimitPerMinute: 60 })
    const q = queries[0]!
    expect(q.values).toEqual([1000, 10.5, 60, 'k1', 'u1'])
  })

  it('scopes the update to the owner AND unrevoked rows; false when nothing matched', async () => {
    updatedRows = 0
    expect(await setKeyPolicy('u1', 'k1', { spendCapTokens: null, spendCapUsd: null, rateLimitPerMinute: null })).toBe(false)
    const q = queries[0]!
    expect(q.text).toContain('user_id =')
    expect(q.text).toContain('revoked_at is null')
  })
})

describe('authenticateKey reads caps on the identity query', () => {
  it('a recognized key carries its ceilings — no second query for policy', async () => {
    keyRow = { id: 'k1', name: 'opencode', userId: 'u1', email: null, spendCapTokens: 5000, spendCapUsd: null, rateLimitPerMinute: 60 }
    const id = await authenticateKey('tlk_real')
    expect(id).toMatchObject({ caps: { tokens: 5000, usd: null, rpm: 60 } })
    // The hot path is two statements total: the lookup, the last_used_at touch.
    expect(queries.filter((q) => q.text.includes('from llm_api_keys'))).toHaveLength(1)
  })

  it('unknown secrets never reach the caps mapping', async () => {
    expect(await authenticateKey('tlk_nope')).toBeNull()
  })
})

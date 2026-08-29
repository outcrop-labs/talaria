import { beforeEach, describe, expect, it, vi } from 'vitest'

// The claim, pinned at the SQL it issues: the advisory lock, the
// in-transaction re-check, the admin upsert, and a password claim's
// credential riding the SAME transaction. Only the edge (db/pg, boards) is
// faked — claimAdmin runs for real, so a change that quietly drops the lock
// or the re-check (the two things standing between a race and a second
// admin) fails here.

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
let adminsExist = false

function answer(text: string): unknown[] {
  if (text.includes('pg_advisory_xact_lock')) return [{ ok: 1 }]
  if (text.includes('not exists') && text.includes("role = 'admin'")) return [{ claimable: !adminsExist }]
  if (text.includes('insert into users')) {
    adminsExist = true
    return [
      { id: 'u1', sub: 'password:jon@talaria.local', email: 'jon@talaria.local', name: 'Jon', picture: null, role: 'admin' },
    ]
  }
  return []
}

const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  return Promise.resolve(answer(text))
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  begin: <T>(fn: (tx: unknown) => Promise<T>) => Promise<T>
}
sql.begin = async <T>(fn: (tx: unknown) => Promise<T>): Promise<T> => fn(sql)

vi.mock('../db/pg', () => ({ db: async () => sql }))
vi.mock('../boards', () => ({ joinOrgWideBoards: vi.fn(async () => {}) }))

const { claimAdmin, instanceClaimable, CLAIM_LOCK } = await import('./claim')
const { joinOrgWideBoards } = await import('../boards')

const identity = { sub: 'google:123', email: 'jon@talaria.local', name: 'Jon', picture: null }

beforeEach(() => {
  queries.length = 0
  adminsExist = false
  vi.mocked(joinOrgWideBoards).mockClear()
})

describe('instanceClaimable', () => {
  it('is true while no admin exists', async () => {
    expect(await instanceClaimable()).toBe(true)
  })

  it('is false once an admin exists', async () => {
    adminsExist = true
    expect(await instanceClaimable()).toBe(false)
  })
})

describe('claimAdmin', () => {
  it('locks, re-checks, upserts the admin — credential in the same transaction', async () => {
    const user = await claimAdmin({ ...identity, sub: 'password:jon@talaria.local' }, 'scrypt$stored-hash')
    expect(user).toMatchObject({ role: 'admin', email: 'jon@talaria.local' })

    const lock = queries.find((q) => q.text.includes('pg_advisory_xact_lock'))
    expect(lock?.values).toEqual([CLAIM_LOCK])

    const upsert = queries.find((q) => q.text.includes('insert into users'))
    expect(upsert?.text).toContain("'admin'")
    expect(upsert?.text).toContain('on conflict (sub)')

    const cred = queries.find((q) => q.text.includes('insert into user_password_credentials'))
    expect(cred?.values).toContain('scrypt$stored-hash')

    // A claim is a sign-in: org-wide boards are joined, best-effort.
    expect(joinOrgWideBoards).toHaveBeenCalledWith('u1')
  })

  it('returns null — and writes nothing — when the race was lost', async () => {
    adminsExist = true
    expect(await claimAdmin(identity, 'scrypt$stored-hash')).toBeNull()
    expect(queries.some((q) => q.text.includes('insert into users'))).toBe(false)
    expect(queries.some((q) => q.text.includes('insert into user_password_credentials'))).toBe(false)
    expect(joinOrgWideBoards).not.toHaveBeenCalled()
  })

  it('a Google claim stores no credential row', async () => {
    await claimAdmin(identity)
    expect(queries.some((q) => q.text.includes('user_password_credentials'))).toBe(false)
  })
})

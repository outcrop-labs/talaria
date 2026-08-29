import { beforeEach, describe, expect, it, vi } from 'vitest'

// The DB-backed credential store, pinned at its queries and its timing story:
// a miss on the email burns the dummy hash (so timing cannot reveal which
// emails have accounts), a hit verifies the stored hash, and the write paths
// refuse with a REASON the route maps to a status — never a thrown 500. The
// hash primitives themselves are mocked (they have their own file's tests);
// everything here is about the table.

vi.mock('./password', () => ({
  hashPassword: vi.fn(async () => 'scrypt$new-hash'),
  verifyPasswordHash: vi.fn(async () => true),
  dummyHash: vi.fn(async () => 'scrypt$dummy-hash'),
}))

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
let account: {
  user_id: string
  email: string
  password_hash: string
  sub: string
  name: string | null
} | null = null
let credEmailTaken = false
let existingUserByEmail: { id: string } | null = null

function answer(text: string): unknown[] {
  if (text.includes('exists(select 1 from user_password_credentials)')) return [{ ok: !!account }]
  if (text.includes('from user_password_credentials c')) return account ? [account] : []
  if (text.includes('select 1 from user_password_credentials where email')) return credEmailTaken ? [1] : []
  if (text.includes('select id from users where lower(email)')) return existingUserByEmail ? [existingUserByEmail] : []
  if (text.includes('insert into users')) return [{ id: 'fresh-user' }]
  if (text.includes('returning email')) return [{ email: 'jon@talaria.local' }]
  if (text.includes('returning user_id')) return [{ user_id: 'u1' }]
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

const { createPasswordAccount, hasPasswordAccounts, verifyPasswordLogin } = await import('./password-accounts')
const { dummyHash, verifyPasswordHash } = await import('./password')

beforeEach(() => {
  queries.length = 0
  account = null
  credEmailTaken = false
  existingUserByEmail = null
  vi.mocked(verifyPasswordHash).mockClear()
  vi.mocked(dummyHash).mockClear()
  vi.mocked(verifyPasswordHash).mockResolvedValue(true)
})

describe('verifyPasswordLogin', () => {
  it('verifies the stored hash and returns the password identity', async () => {
    account = { user_id: 'u1', email: 'jon@talaria.local', password_hash: 'scrypt$stored', sub: 'password:jon@talaria.local', name: 'Jon' }
    const identity = await verifyPasswordLogin('JON@TALARIA.LOCAL  ', 'hunter2')
    expect(identity).toMatchObject({ provider: 'password', sub: 'password:jon@talaria.local', email: 'jon@talaria.local', name: 'Jon' })
    expect(verifyPasswordHash).toHaveBeenCalledWith('hunter2', 'scrypt$stored')
    // The activity stamp is fire-and-forget, but it fires.
    expect(queries.some((q) => q.text.includes('set last_used_at = now()'))).toBe(true)
  })

  it('an unknown email burns the dummy hash — a miss on the email costs a full verify', async () => {
    expect(await verifyPasswordLogin('nobody@talaria.local', 'hunter2')).toBeNull()
    expect(dummyHash).toHaveBeenCalled()
    expect(verifyPasswordHash).toHaveBeenCalledWith('hunter2', 'scrypt$dummy-hash')
  })

  it('a wrong password fails without the activity stamp', async () => {
    account = { user_id: 'u1', email: 'jon@talaria.local', password_hash: 'scrypt$stored', sub: 'password:jon@talaria.local', name: null }
    vi.mocked(verifyPasswordHash).mockResolvedValue(false)
    expect(await verifyPasswordLogin('jon@talaria.local', 'wrong')).toBeNull()
    expect(queries.some((q) => q.text.includes('set last_used_at = now()'))).toBe(false)
  })
})

describe('hasPasswordAccounts', () => {
  it('follows the table — false empty, true with a row', async () => {
    expect(await hasPasswordAccounts()).toBe(false)
    account = { user_id: 'u1', email: 'j@t.local', password_hash: 'scrypt$stored', sub: 'password:j@t.local', name: null }
    expect(await hasPasswordAccounts()).toBe(true)
  })
})

describe('createPasswordAccount', () => {
  it('creates a member users row for a brand-new email, keyed password:<email>', async () => {
    const result = await createPasswordAccount({ email: 'New@Talaria.Local ', password: 'hunter2!' })
    expect(result).toEqual({ ok: true, userId: 'fresh-user' })
    const insert = queries.find((q) => q.text.includes('insert into users'))
    expect(insert?.values).toContain('password:new@talaria.local')
    expect(queries.some((q) => q.text.includes('insert into user_password_credentials'))).toBe(true)
  })

  it('attaches to the EXISTING person when their email already has a users row', async () => {
    existingUserByEmail = { id: 'existing-u' }
    const result = await createPasswordAccount({ email: 'jon@talaria.local', password: 'hunter2!' })
    expect(result).toEqual({ ok: true, userId: 'existing-u' })
    expect(queries.some((q) => q.text.includes('insert into users'))).toBe(false)
  })

  it('refuses with a reason when the email already belongs to an account', async () => {
    credEmailTaken = true
    expect(await createPasswordAccount({ email: 'jon@talaria.local', password: 'hunter2!' })).toEqual({
      ok: false,
      reason: 'email-taken',
    })
  })
})

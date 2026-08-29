import { beforeEach, describe, expect, it, vi } from 'vitest'

// The two listUsersAdmin/upsertUser behaviors the claim flow changed:
// hasPasswordAccount arrives from the SQL (no post-map, no pinnedAdmin), and
// adminCount feeds the last-admin guard. Only the db edge is faked.

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
let countAnswer = 0
let userRows: unknown[] = []

function answer(text: string): unknown[] {
  if (text.includes('count(*)')) return [{ n: countAnswer }]
  if (text.includes('from users u')) return userRows
  if (text.includes('insert into users')) return [{ id: 'u1', sub: 'google:1', email: 'j@t.local', name: 'J', picture: null, role: 'member' }]
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

vi.mock('./db/pg', () => ({ db: async () => sql }))
vi.mock('./boards', () => ({ joinOrgWideBoards: vi.fn(async () => {}) }))

const { adminCount, listUsersAdmin, upsertUser } = await import('./users')

beforeEach(() => {
  queries.length = 0
  countAnswer = 0
  userRows = []
})

describe('adminCount', () => {
  it('reads the count the last-admin guard keys on', async () => {
    countAnswer = 1
    expect(await adminCount()).toBe(1)
  })

  it('treats a missing count row as zero — a number the guard can trust', async () => {
    // count(*) always returns a row; if that ever changes shape, the ?? 0
    // keeps the last-admin guard reading a real number instead of NaN.
    expect(await adminCount()).toBe(0)
  })
})

describe('listUsersAdmin', () => {
  it('selects hasPasswordAccount in the SQL — the rows pass through unmapped', async () => {
    userRows = [{ id: 'u1', email: 'j@t.local', hasPasswordAccount: true }]
    const users = await listUsersAdmin()
    expect(users[0]).toMatchObject({ id: 'u1', hasPasswordAccount: true })
    expect(queries[0]?.text).toContain('hasPasswordAccount')
    // The old env-pinned map is gone: no AUTH_ADMIN_EMAILS read, no re-shape.
    expect(users.length).toBe(1)
  })
})

describe('upsertUser', () => {
  it('inserts members and never touches role on conflict — no env promotion', async () => {
    const user = await upsertUser({ sub: 'google:1', email: 'j@t.local', name: 'J', picture: null })
    expect(user.role).toBe('member')
    const insert = queries.find((q) => q.text.includes('insert into users'))
    expect(insert?.text).toContain("'member'")
    // The old promote case (role = case when … then 'admin') must be gone.
    expect(insert?.text).not.toContain("role = case")
  })
})

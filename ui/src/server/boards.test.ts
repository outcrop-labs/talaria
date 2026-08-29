// WHERE THE PROMISE AND THE GATE MEET.
//
// GET /api/boards tells a personal assistant "role: owner" on its owner's
// boards — on purpose, so it can govern them on the owner's behalf. Every
// board-scoped route answers the agent ALLOWLIST instead. Those two must not
// disagree: an agent told it owns a board it can only ever 403 against is the
// defect the 2026-08-28 sweep filed, and the fix is propagation, not a quieter
// read path — the board is BORN carrying the owner's personal assistant on its
// allowlist (the other direction, an assistant born after the boards, is pinned
// in personal-agent-bootstrap.test.ts).
import { beforeEach, describe, expect, it, vi } from 'vitest'

const queries: Array<{ text: string; values: unknown[] }> = []

// Query-aware: the boards INSERT must return the row createBoard shapes its
// answer from; every other statement may answer empty.
const exec = (strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  if (/^insert into boards /.test(text)) {
    return Promise.resolve([
      { id: 'b1', name: 'Fake Widget Marketing Plan', ownerId: 'u1', teamId: null, createdAt: 't', updatedAt: 't' },
    ])
  }
  return Promise.resolve([])
}
const sql = Object.assign(exec, {
  begin: async (fn: (tx: unknown) => Promise<unknown>) => fn(exec),
  unsafe: (text: string) => {
    queries.push({ text: text.replace(/\s+/g, ' ').trim(), values: [] })
    return Promise.resolve([])
  },
  json: (v: unknown) => v,
})

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/users', () => ({ isElevatedAssistant: async () => false }))
vi.mock('@/server/statuses', () => ({ statusMeta: (s: string) => ({ label: s }) }))

const { boardAllowsAgent, createBoard } = await import('@/server/boards')

beforeEach(() => {
  queries.length = 0
})

describe('createBoard', () => {
  it('is born with the owner’s personal assistant on the agent allowlist', async () => {
    const board = await createBoard('u1', 'Fake Widget Marketing Plan')
    expect(board.role).toBe('owner')

    // Three writes, in order, inside the one transaction: the board, the
    // owner's membership row, and the assistant's allowlist row. No window
    // where the read path lists the board and the write path refuses it.
    const writes = queries.filter((q) => /^insert into /.test(q.text)).map((q) => q.text)
    expect(writes).toHaveLength(3)
    expect(writes[1]).toContain('insert into board_members')
    expect(writes[2]).toContain('insert into board_agents')
    // Seeded FROM ownership: the owner's row in agent_defs, this creator's id.
    expect(writes[2]).toContain('from agent_defs where owner_user_id =')
    expect(queries[queries.length - 1]?.values).toContain('u1')
    expect(writes[2]).toContain('on conflict do nothing')
  })
})

describe('boardAllowsAgent', () => {
  const facts = (models: string[]) => ({
    info: async () => ({ label: 'b1', archivedAt: null as string | null, exists: true }),
    policy: async () => ({ allowAll: false, models }),
    meta: async () => ({ label: 'inbox' }) as never,
  })

  it('admits a listed agent', async () => {
    expect(await boardAllowsAgent('b1', 'gregosaurus-personal-gregosaurus', facts(['gregosaurus-personal-gregosaurus']))).toBe(true)
  })

  it('refuses an unlisted one — ownership does NOT bypass the allowlist', async () => {
    // The propagation lives in the seeding writes above, not in the gate: a
    // deliberate removal via set_board_agents must stick, so the check stays
    // blind to who owns the board.
    expect(await boardAllowsAgent('b1', 'gregosaurus-personal-gregosaurus', facts([]))).toBe(false)
  })
})

// THE DOORS THE PORT MISSED. `agent-writes.ts` calls itself "the one door for
// agent-authored text on its way to a human" and names three MCP tools. There
// were five. `create_ticket` / `triage_ticket` / `report_outcome` write a title,
// a description, an outcome and a resolution, and `report_gap` writes the one
// piece of agent prose whose SUBJECT is "the access and credentials I am
// missing". None of them was scanned: the text went raw into the `tasks` row, on
// into `indexTicket` (which another agent then reaches through
// `search_knowledge`), out through `notifyMentions` to a human's inbox and mail
// — and no `guard_findings` row was ever written, so the fitness page
// undercounted the most-used agent write surface in the product.
//
// Same method as agent-writes.wiring.test.ts: only the edges are faked, so
// `guardrails.ts` and `agent-writes.ts` run for real and a change that quietly
// stops calling the door fails here.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { GuardMode } from '@/server/guardrails'

const CRED = `github_pat_11ABCDEFG0${'aBcD1234_'.repeat(4)}`
const AGENT = 'nomad'

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
let mode: GuardMode = 'strict'

function answer(text: string, values: unknown[]): unknown[] {
  if (text.includes('from app_settings')) return [{ value: { mode }, memo: null }]
  if (text.includes('select 1 from agent_defs')) return values[0] === AGENT ? [{ ok: 1 }] : []
  if (text.includes('update boards set ticket_seq')) return [{ ticket_seq: 12 }]
  if (text.includes('insert into tasks')) return [{ id: 'task-1' }]
  if (text.includes('insert into capability_gaps')) return [{ id: 'gap-1', seenCount: 1, first: false }]
  return []
}

const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  return Promise.resolve(answer(text, values))
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  json: (v: unknown) => unknown
  begin: <T>(fn: (tx: unknown) => Promise<T>) => Promise<T>
  unsafe: (text: string, values?: unknown[]) => Promise<unknown[]>
}
sql.json = (v: unknown) => v
sql.begin = async <T,>(fn: (tx: unknown) => Promise<T>): Promise<T> => fn(sql)
// `getTask` reads through `sql.unsafe`; the row it hands back is only used for
// the dispatch/notify tail, none of which is the subject here.
sql.unsafe = async () => [{ id: 'task-1', boardId: 'board-1', title: 't', assignees: [], status: 'intake' }]

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/realtime', () => ({ publishBoard: () => {}, publishChannel: () => {} }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/usage', () => ({ estimateTokens: () => 0, recordUsage: async () => {}, taskUsage: async () => null }))
vi.mock('@/server/notifications', () => ({ addNotification: async () => {} }))
vi.mock('@/server/labels', () => ({ ensureLabels: async () => {} }))
vi.mock('@/server/judge', () => ({ listJudgeReviews: async () => [] }))
vi.mock('@/server/work-dispatch', () => ({ maybeDispatchTicket: async () => {} }))
vi.mock('@/server/statuses', async () => ({
  OFF_BOARD_STATUSES: (await import('@/lib/task-const')).OFF_BOARD_STATUSES,
  statusMeta: async () => ({ keys: ['intake'], defaultKey: 'intake', assignedKey: null }),
}))
vi.mock('@/server/approvals', () => ({ audienceFor: async () => ({ content: [], fact: [] }) }))

const { createTask } = await import('@/server/tasks')
const { reportGap } = await import('@/server/gaps')

beforeEach(() => {
  queries.length = 0
  mode = 'strict'
})

const inserted = (table: string): Query | undefined => queries.find((q) => q.text.includes(`insert into ${table}`))
const findings = () => queries.filter((q) => q.text.includes('insert into guard_findings'))

// ── mcp `create_ticket` → tasks.createTask ───────────────────────────────────

describe('agent-authored tickets', () => {
  it('redacts a credential out of the description before the row is written', async () => {
    await createTask({ boardId: 'board-1', title: 'Rotate the deploy key', description: `The current key is ${CRED} — @priya please rotate it.`, createdBy: AGENT })
    const values = inserted('tasks')?.values ?? []
    expect(values.some((v) => String(v).includes(CRED))).toBe(false)
    expect(values.some((v) => String(v).includes('[redacted GitHub fine-grained token]'))).toBe(true)
  })

  it('redacts a credential out of the TITLE too, because that is what a notification carries', async () => {
    await createTask({ boardId: 'board-1', title: `Rotate ${CRED}`, description: 'nothing here', createdBy: AGENT })
    expect((inserted('tasks')?.values ?? []).some((v) => String(v).includes(CRED))).toBe(false)
  })

  it('files ONE finding against the agent and names the write path', async () => {
    await createTask({ boardId: 'board-1', title: 'Rotate the deploy key', description: `key ${CRED}`, createdBy: AGENT })
    // One pass over the fields, so a ticket is one row rather than four.
    expect(findings()).toHaveLength(1)
    const values = findings()[0]?.values ?? []
    expect(values).toContain(AGENT)
    expect(values.some((v) => String(v).includes('ticket-write'))).toBe(true)
  })

  it('records without altering in observe mode, which is the org default', async () => {
    mode = 'observe'
    await createTask({ boardId: 'board-1', title: 'Rotate the deploy key', description: `key ${CRED}`, createdBy: AGENT })
    expect((inserted('tasks')?.values ?? []).some((v) => String(v).includes(CRED))).toBe(true)
    expect(findings()).toHaveLength(1)
  })

  it('leaves a HUMAN ticket exactly as typed', async () => {
    await createTask({ boardId: 'board-1', title: 'Rotate the deploy key', description: `key ${CRED}`, createdBy: 'priya@example.com' })
    expect((inserted('tasks')?.values ?? []).some((v) => String(v).includes(CRED))).toBe(true)
    expect(findings()).toHaveLength(0)
  })

  it('leaves an ordinary agent ticket untouched', async () => {
    await createTask({ boardId: 'board-1', title: 'Rotate the deploy key', description: 'Nadia owns the rollback plan.', createdBy: AGENT })
    expect((inserted('tasks')?.values ?? []).some((v) => String(v).includes('Nadia owns the rollback plan.'))).toBe(true)
    expect(findings()).toHaveLength(0)
  })
})

// ── mcp `report_gap` → gaps.reportGap ────────────────────────────────────────

describe('capability gaps', () => {
  it('redacts a credential out of the gap before it is stored and announced', async () => {
    await reportGap({ agentModel: AGENT, kind: 'deploy', missing: `no way to rotate the key ${CRED}`, needs: `the key ${CRED} is still live` })
    const values = inserted('capability_gaps')?.values ?? []
    expect(values.some((v) => String(v).includes(CRED))).toBe(false)
    expect(values.filter((v) => String(v).includes('[redacted GitHub fine-grained token]'))).toHaveLength(2)
    expect(findings()).toHaveLength(1)
  })

  it('leaves an ordinary gap report alone', async () => {
    await reportGap({ agentModel: AGENT, kind: 'deploy', missing: 'no way to rotate the deploy key', needs: 'a rotation tool' })
    expect((inserted('capability_gaps')?.values ?? []).some((v) => String(v).includes('no way to rotate the deploy key'))).toBe(true)
    expect(findings()).toHaveLength(0)
  })
})

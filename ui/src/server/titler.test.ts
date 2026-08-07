// THE GATE AND THE WRITE ARE SECONDS APART, and a model call sits between them.
// `sweepTitles` snapshots up to 100 conversations, decides which are still
// wearing the mechanical first-message truncation, and then makes up to twelve
// SEQUENTIAL model calls against that snapshot. A rename landing in the gap was
// overwritten — and permanently, because a model-written title no longer matches
// the mechanical truncation, so neither retitle path ever revisits that row
// again. The rename affordance is on the same screen whose channel read kicks
// the sweep.
import { beforeEach, describe, expect, it, vi } from 'vitest'

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
/** The stored title, mutated mid-call to stand in for the concurrent rename. */
let stored: string | null = 'how do I rotate the certs on the edge'
let renameDuringCall: string | null = null

const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  if (text.startsWith('update conversations set title')) {
    // The optimistic predicate, enforced the way postgres would: the row is
    // written only if the title is still the one the gate approved.
    const expected = values[1] === 'conv-1' ? values[2] : values[1]
    if (expected === stored) stored = String(values[0])
    return Promise.resolve([])
  }
  if (text.includes('from conversations c where c.id')) {
    return Promise.resolve([{ title: stored, kind: 'chat', count: 2 }])
  }
  if (text.includes('from conversations c')) {
    return Promise.resolve([{ id: 'conv-1', title: stored, kind: 'chat', first: 'how do I rotate the certs on the edge' }])
  }
  if (text.includes('from messages')) {
    return Promise.resolve([
      { role: 'user', content: 'how do I rotate the certs on the edge' },
      { role: 'assistant', content: 'run the rotation script' },
    ])
  }
  return Promise.resolve([])
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  json: (v: unknown) => unknown
}
sql.json = (v: unknown) => v

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/harness/run', () => ({
  runHarness: async () => {
    // The window: the model is thinking, and the user renames the chat.
    if (renameDuringCall !== null) stored = renameDuringCall
    return { value: 'Rotating Edge Certificates', model: 'pl-main', step: 'pin', widened: false, repairs: 0, schemaValid: true, findings: [], raw: 'x', latencyMs: 1, escalate: false }
  },
}))

const { maybeRetitleConversation, sweepTitles } = await import('@/server/titler')

beforeEach(() => {
  queries.length = 0
  stored = 'how do I rotate the certs on the edge'
  renameDuringCall = null
})

describe('the retitle write', () => {
  it('names a chat that is still wearing its first message', async () => {
    await maybeRetitleConversation('conv-1')
    expect(stored).toBe('Rotating Edge Certificates')
  })

  it('leaves a title the user typed while the model was thinking', async () => {
    renameDuringCall = 'CERT ROTATION — DO NOT DELETE'
    await maybeRetitleConversation('conv-1')
    expect(stored).toBe('CERT ROTATION — DO NOT DELETE')
  })

  it('leaves it alone on the sweep path too, where the window is minutes wide', async () => {
    renameDuringCall = 'CERT ROTATION — DO NOT DELETE'
    await sweepTitles()
    expect(stored).toBe('CERT ROTATION — DO NOT DELETE')
  })

  it('still names it on the sweep path when nobody renamed it', async () => {
    await sweepTitles()
    expect(stored).toBe('Rotating Edge Certificates')
  })
})

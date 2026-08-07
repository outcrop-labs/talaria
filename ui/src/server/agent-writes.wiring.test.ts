import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { GuardMode } from '@/server/guardrails'

// THE WIRING, which is the half a unit test of the door cannot prove. The hole
// the audit found was not "no guard exists" — it was that three write paths
// reached a human without going through the one that did. So these cases drive
// the REAL `addComment`, the REAL `insertChannelMessage` and the REAL
// `agentMessageUser` against a fake database and assert on the bytes that reach
// the insert.
//
// Only the edges are faked: postgres, the realtime bus, the conversation store
// and the notifier. `guardrails.ts` and `agent-writes.ts` run for real, so a
// change that quietly stops calling the door fails here.

const CRED = `tak_${'a'.repeat(40)}`
const AGENT = 'nomad'

interface Query {
  text: string
  values: unknown[]
}
const queries: Query[] = []
let mode: GuardMode = 'strict'

function answer(text: string, values: unknown[]): unknown[] {
  if (text.includes('from app_settings')) return [{ value: { mode } }]
  if (text.includes('select 1 from agent_defs')) return values[0] === AGENT ? [{ ok: 1 }] : []
  if (text.includes('from agent_defs')) return [{ model: AGENT, displayName: 'Nomad', ownerUserId: null }]
  if (text.includes('from users')) return [{ id: 'user-1', email: 'priya@example.com', name: 'Priya' }]
  if (text.includes('count(*)') && text.includes('from outreach_events')) return [{ count: 0 }]
  if (text.includes('update channels set msg_seq')) return [{ msg_seq: 7 }]
  if (text.includes('insert into channel_messages')) {
    return [{ id: 'chan-msg-1', seq: 7, authorType: values[2], author: values[3], content: values[4], status: values[5], createdAt: 'now', threadRootId: null }]
  }
  if (text.includes('insert into task_comments')) {
    return [{ id: 'comment-1', author: values[1], content: values[2], parentId: null, createdAt: 'now' }]
  }
  if (text.includes('select board_id from tasks')) return [{ board_id: 'board-1' }]
  return []
}

/** A postgres.js-shaped tagged template that answers by inspecting the query
 *  text and records the interpolated VALUES — which is where the guarded body
 *  actually shows up. (comms-decay.test.ts's fake, plus the values.) */
const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  return Promise.resolve(answer(text, values))
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  json: (v: unknown) => unknown
  begin: <T>(fn: (tx: unknown) => Promise<T>) => Promise<T>
}
sql.json = (v: unknown) => v
sql.begin = async <T,>(fn: (tx: unknown) => Promise<T>): Promise<T> => fn(sql)

const persisted = { assistant: '', notification: '', guardPinned: 0 }

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/realtime', () => ({ publishChannel: () => {}, publishBoard: () => {} }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/usage', () => ({ estimateTokens: () => 0, recordUsage: async () => {}, taskUsage: async () => null }))
vi.mock('@/server/harness/run', () => ({ runHarness: async () => ({ value: null }) }))
vi.mock('@/server/gateway', () => ({ describeAgent: (id: string) => ({ label: id }), proxyChat: async () => ({ ok: false }) }))
vi.mock('@/server/notifications', () => ({
  addNotification: async (_userId: string, n: { body: string }) => {
    persisted.notification = n.body
  },
}))
vi.mock('@/server/conversations', () => ({
  createConversation: async () => 'conv-1',
  nextSeq: async () => 1,
  insertStreamingAssistant: async () => 'msg-1',
  updateAssistant: async (_id: string, data: { content: string }) => {
    persisted.assistant = data.content
  },
  setMessageGuard: async () => {
    persisted.guardPinned++
  },
  touchConversation: async () => {},
}))

const { addComment } = await import('@/server/tasks')
const { insertChannelMessage } = await import('@/server/channels')
const { agentMessageUser } = await import('@/server/outreach')

beforeEach(() => {
  queries.length = 0
  mode = 'strict'
  persisted.assistant = ''
  persisted.notification = ''
  persisted.guardPinned = 0
})

const inserted = (table: string, index: number): string => {
  const q = queries.find((x) => x.text.includes(`insert into ${table}`))
  return String(q?.values[index] ?? '')
}
const findings = () => queries.filter((q) => q.text.includes('insert into guard_findings'))

// ── mcp `comment` → tasks.addComment ─────────────────────────────────────────

describe('ticket comments', () => {
  it('redacts a credential out of an agent comment before it is stored', async () => {
    await addComment('task-1', AGENT, `the workbench token is ${CRED}`)
    expect(inserted('task_comments', 2)).not.toContain(CRED)
    expect(inserted('task_comments', 2)).toContain('[redacted Talaria agent credential]')
    expect(findings()).toHaveLength(1)
  })

  it('files the finding against the agent and names the write path', async () => {
    await addComment('task-1', AGENT, `token: ${CRED}`)
    expect(findings()[0]?.values.slice(0, 4)).toEqual(['ticket-comment:nomad', AGENT, 'fleet', 'strict'])
  })

  it('leaves a human comment exactly as typed', async () => {
    // A person pasting a key into a ticket is a different problem with a
    // different owner. Guarding it here would also file the finding against a
    // "model" that is an email address.
    const text = `here is the rotated key: ${CRED}`
    await addComment('task-1', 'priya@example.com', text)
    expect(inserted('task_comments', 2)).toBe(text)
    expect(findings()).toHaveLength(0)
  })

  it('leaves an ordinary agent comment untouched', async () => {
    const text = 'Rebased onto main; the failing spec was a fixture path, fixed in the same commit.'
    await addComment('task-1', AGENT, text)
    expect(inserted('task_comments', 2)).toBe(text)
    expect(findings()).toHaveLength(0)
  })
})

// ── mcp `post_to_channel` → channels.insertChannelMessage ────────────────────

describe('channel posts', () => {
  it('redacts a credential out of an agent post before it is stored', async () => {
    await insertChannelMessage('chan-1', 'agent', AGENT, `deploy key: ${CRED}`)
    expect(inserted('channel_messages', 4)).not.toContain(CRED)
    expect(findings()[0]?.values[0]).toBe('channel-post:nomad')
  })

  it('does not guard a human post', async () => {
    const text = `deploy key: ${CRED}`
    await insertChannelMessage('chan-1', 'user', 'priya@example.com', text)
    expect(inserted('channel_messages', 4)).toBe(text)
    expect(findings()).toHaveLength(0)
  })

  it('costs the streamed reply path nothing — that row is inserted empty', async () => {
    // channel-replies.ts inserts a placeholder and fills it in through
    // updateChannelMessage, where guardChatReply already stands. Guarding an
    // empty string would be a second findings row for one reply, and
    // guard_findings.model is a rate the fitness page reads.
    await insertChannelMessage('chan-1', 'agent', AGENT, '', 'streaming')
    expect(queries.some((q) => q.text.includes('from app_settings'))).toBe(false)
    expect(findings()).toHaveLength(0)
  })
})

// ── mcp `message_user` → outreach.agentMessageUser ───────────────────────────

describe('direct messages', () => {
  it('redacts the persisted turn, the inbox notification and the outreach note', async () => {
    const res = await agentMessageUser(AGENT, 'priya@example.com', `you'll need ${CRED} for staging`)
    expect(res.ok).toBe(true)
    expect(persisted.assistant).not.toContain(CRED)
    expect(persisted.notification).not.toContain(CRED)
    expect(inserted('outreach_events', 4)).not.toContain(CRED)
    expect(findings()[0]?.values.slice(0, 2)).toEqual(['direct-message:nomad', AGENT])
  })

  it('pins the findings onto the message in strict mode, as chat replies already do', async () => {
    await agentMessageUser(AGENT, 'priya@example.com', `key ${CRED}`)
    expect(persisted.guardPinned).toBe(1)
  })

  it('records without altering in observe mode — but still scrubs the note the agent re-reads', async () => {
    // `checkInTurn` feeds recent outreach notes back into the agent's next
    // prompt. That is a model context, so guardrails.ts's cardinal invariant
    // applies to the note in EVERY mode, not only the one that rewrites the
    // message a human sees.
    mode = 'observe'
    const text = `you'll need ${CRED} for staging`
    await agentMessageUser(AGENT, 'priya@example.com', text)

    expect(persisted.assistant).toBe(text)
    expect(persisted.notification).toBe(text)
    expect(inserted('outreach_events', 4)).not.toContain(CRED)
    expect(findings()).toHaveLength(1)
    expect(persisted.guardPinned).toBe(0)
  })

  it('leaves a clean message alone end to end', async () => {
    const text = 'ALPH-14 is blocked on you — the staging credentials expired overnight.'
    await agentMessageUser(AGENT, 'priya@example.com', text)

    expect(persisted.assistant).toBe(text)
    expect(persisted.notification).toBe(text)
    expect(inserted('outreach_events', 4)).toBe(text)
    expect(findings()).toHaveLength(0)
  })
})

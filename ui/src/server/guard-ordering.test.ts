// THE ORDER THE GUARD RUNS IN, which is the whole of the fix and invisible to a
// unit test of either half.
//
// Both of these paths scrubbed the message row in strict mode and had ALREADY
// handed the unredacted text to something that keeps its own copy:
//
//   chat-persist.ts   `indexActivity` put the raw reply into the plan owner's
//                     retrieval collection — which nothing ever re-indexes, and
//                     which `search_knowledge` serves back to a model. That is
//                     the one thing guardrails.ts's cardinal invariant forbids.
//                     `notifyPlanMentions` mailed it in the same breath.
//   channel-replies.ts `notifyMentions` wrote it into a `notifications` row
//                     (read back into the Inbox Focus brief) and out through
//                     gated mail. Nothing ever scrubs that row.
//
// The already-fixed siblings — channels.$id.messages.ts and
// tasks.$id.comments.ts — index and notify from the POST-guard text and say so
// in a comment. These two are the same shape, so they are asserted the same way.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const CRED = `tak_${'a'.repeat(40)}`
const REPLY = `I saved the rotation runbook with the key ${CRED} for @priya.`

const indexed: string[] = []
const notified: string[] = []
const rows: string[] = []
const pinnedSnippets: string[] = []

vi.mock('@/server/db/pg', () => ({ db: async () => Object.assign(() => Promise.resolve([]), { json: (v: unknown) => v }) }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/usage', () => ({ estimateTokens: () => 0, recordUsage: async () => {} }))
vi.mock('@/server/fleet-agents', () => ({ routedModelFor: async () => null }))
vi.mock('@/server/gateway', () => ({
  describeAgent: (id: string) => ({ label: id }),
  proxyChat: async () => new Response(sse(REPLY), { headers: { 'Content-Type': 'text/event-stream' } }),
}))
vi.mock('@/server/users', () => ({ listUsers: async () => [], personalAssistantOwners: async () => new Map() }))
vi.mock('@/server/refs', () => ({ refBlocks: async () => [] }))
vi.mock('@/server/uploads', () => ({
  attachmentAsDataUrl: async () => null,
  attachmentTextBlocks: async () => [],
  isImage: () => false,
}))
vi.mock('@/server/notifications', () => ({ addNotification: async () => {} }))
// The guard itself is REAL below; only the org's posture is supplied.
vi.mock('@/server/audit', () => ({ getSetting: async () => ({ mode: 'strict' }), logAudit: async () => {} }))
vi.mock('@/server/retrieval/sources', () => ({
  indexActivity: async (doc: { text: string }) => {
    indexed.push(doc.text)
  },
}))
vi.mock('@/server/plan-doc', () => ({
  PLAN_MODE_PROMPT: '',
  planRoutingBlock: async () => '',
  notifyPlanMentions: async (_id: string, _by: unknown, content: string) => {
    notified.push(content)
  },
}))
vi.mock('@/server/mentions', () => ({
  notifyMentions: async (_members: unknown, _id: string, _label: string, content: string) => {
    notified.push(content)
  },
}))
vi.mock('@/server/titler', () => ({ maybeRetitleConversation: async () => {} }))
vi.mock('@/server/conversations', () => ({
  activeStreamingAssistant: async () => false,
  insertStreamingAssistant: async () => 'msg-1',
  nextSeq: async () => 1,
  priorMessages: async () => [],
  touchConversation: async () => {},
  updateAssistant: async (_id: string, data: { content: string }) => {
    rows.push(data.content)
  },
  setMessageGuard: async (_id: string, findings: Array<{ snippet: string }>) => {
    pinnedSnippets.push(...findings.map((f) => f.snippet))
  },
}))
vi.mock('@/server/channels', () => ({
  listChannelAgents: async () => ['nomad'],
  listChannelMembers: async () => [{ id: 'user-1', name: 'Priya', email: 'priya@example.com' }],
  listChannelMessages: async () => [],
  listThreadMessages: async () => [],
  insertChannelMessage: async () => ({ id: 'msg-1' }),
  updateChannelMessage: async (_c: string, _m: string, content: string) => {
    rows.push(content)
  },
  setChannelMessageGuard: async (_c: string, _m: string, findings: Array<{ snippet: string }>, content?: string) => {
    pinnedSnippets.push(...findings.map((f) => f.snippet))
    if (content !== undefined) rows.push(content)
  },
}))

const { persistAssistantStream } = await import('@/server/chat-persist')
const { triggerAgentReplies } = await import('@/server/channel-replies')

/** One SSE frame carrying the whole reply, in the shape `parseAgentStream` reads. */
function sse(text: string): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(c) {
      c.enqueue(new TextEncoder().encode(`data: ${JSON.stringify({ choices: [{ delta: { content: text } }] })}\n\ndata: [DONE]\n\n`))
      c.close()
    },
  })
}
const stream = sse
/** The channel reply is kicked detached, so let its promise chain drain. */
const settle = async () => {
  for (let i = 0; i < 50; i++) await Promise.resolve()
  await new Promise((r) => setTimeout(r, 5))
}

beforeEach(() => {
  indexed.length = 0
  notified.length = 0
  rows.length = 0
  pinnedSnippets.length = 0
})

describe('a plan reply', () => {
  it('indexes and notifies the REDACTED text, not the raw one', async () => {
    await persistAssistantStream(stream(REPLY), 'msg-1', 'conv-1', {
      agentModel: 'nomad',
      promptChars: 10,
      plan: { ownerUserId: 'user-1', title: 'Ledger migration' },
    })

    expect(rows.at(-1)).not.toContain(CRED)
    expect(indexed).toHaveLength(1)
    expect(indexed[0]).not.toContain(CRED)
    expect(indexed[0]).toContain('[redacted Talaria agent credential]')
    expect(notified).toHaveLength(1)
    expect(notified[0]).not.toContain(CRED)
  })

  it('scrubs the snippet it pins, which is a verbatim excerpt of the same span', async () => {
    await persistAssistantStream(stream(REPLY), 'msg-1', 'conv-1', {
      agentModel: 'nomad',
      promptChars: 10,
      plan: { ownerUserId: 'user-1', title: 'Ledger migration' },
    })
    expect(pinnedSnippets.length).toBeGreaterThan(0)
    for (const s of pinnedSnippets) expect(s).not.toContain(CRED)
  })
})

describe('a channel agent reply', () => {
  it('notifies the REDACTED text, not the raw one', async () => {
    await triggerAgentReplies('chan-1', 'platform', 'hey @nomad what is the key?')
    await settle()

    expect(rows.at(-1)).not.toContain(CRED)
    expect(notified).toHaveLength(1)
    expect(notified[0]).not.toContain(CRED)
    expect(notified[0]).toContain('[redacted Talaria agent credential]')
  })
})

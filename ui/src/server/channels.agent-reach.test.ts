import { beforeEach, describe, expect, it, vi } from 'vitest'

// The reach a personal assistant has through the MCP toolkit's channel tools.
//
// The bug this file pins down: the briefing chat asks about the OWNER's
// comms ("Priya mentioned you in #platform"), but `list_channels` answered
// with the AGENT's memberships — and nobody adds a personal assistant to
// channels; its owner is the member. Empty listing, then a model guessing the
// channel NAME as an id, then a uuid-cast 500 out of the database. The fix is
// the identity-proxy model the rest of the product already uses: a proven
// personal assistant sees its owner's channels, and a non-uuid id is answered
// by the predicate instead of thrown by Postgres.
//
// The database and the users-module predicates are faked; every assertion is
// about THIS file's branch order and its query shapes.

const net = vi.hoisted(() => ({
  /** Every query this module issued, as normalized text — the branch taken. */
  queries: [] as string[],
  /** channel_members rows by (channelId, userId). */
  memberships: new Map<string, string>(),
  /** The owner a model resolves to, by model name. */
  owners: new Map<string, string>(),
  /** channel_agents rows: channelId → models. */
  agentMemberships: new Map<string, string[]>(),
}))

vi.mock('@/server/db/pg', () => ({
  db: async () => (strings: TemplateStringsArray, ...values: unknown[]) => {
    const text = strings.join(' ').replace(/\s+/g, ' ').trim()
    net.queries.push(text)
    if (text.includes('from agent_defs d')) {
      const model = values[0] as string
      const owner = net.owners.get(model)
      return Promise.resolve(owner ? [{ id: owner }] : [])
    }
    if (text.includes('from channel_agents')) {
      const channelId = values[0] as string
      return Promise.resolve((net.agentMemberships.get(channelId) ?? []).map((agent_model) => ({ agent_model })))
    }
    if (text.includes('select role from channel_members')) {
      // values: [channelId, userId]
      const role = net.memberships.get(`${values[0]}:${values[1]}`)
      return Promise.resolve(role ? [{ role }] : [])
    }
    // listChannels' big membership join (the owner view) and everything else.
    if (text.includes('from channels c') && text.includes('join channel_members')) {
      const userId = values[0] as string
      return Promise.resolve(
        [...net.memberships.entries()]
          .filter(([k]) => k.endsWith(`:${userId}`))
          .map(([k, role]) => ({
            id: k.split(':')[0]!,
            name: k.split(':')[0]!.startsWith('11111111') ? 'platform' : k.split(':')[0]!.startsWith('22222222') ? 'dm-with-priya' : 'other',
            topic: null,
            kind: k.split(':')[0]!.startsWith('22222222') ? 'dm' : 'channel',
            role,
            createdAt: '2026-08-20T09:00:00Z',
            updatedAt: '2026-08-20T09:00:00Z',
            peerId: k.split(':')[0]!.startsWith('22222222') ? 'u-priya' : null,
            peerName: k.split(':')[0]!.startsWith('22222222') ? 'Priya' : null,
            peerEmail: null,
            unreadCount: 3,
          })),
      )
    }
    return Promise.resolve([])
  },
}))
vi.mock('@/server/realtime', () => ({ publishChannel: () => {}, publishUser: () => {} }))
vi.mock('@/server/daily-brief-stale', () => ({ briefsFollowMessage: () => {}, markBriefStale: async () => {} }))
vi.mock('@/server/agent-writes', () => ({ guardAgentWrite: async (_t: string, body: string) => body }))
vi.mock('@/server/users', () => ({
  isElevatedAssistant: async () => false,
  assistantOwnerId: async (subject: unknown) => {
    // The REAL predicate's one rule that matters here: owner reach demands the
    // resolved CALLER — never a bare model string, never a legacy one.
    const s = subject as { model: string; legacy: boolean }
    if (typeof subject === 'string' || s.legacy) return null
    return net.owners.get(s.model) ?? null
  },
}))

const { channelRole, agentMayAccessChannel, listChannelsForAgent } = await import('@/server/channels')

const OWNER = 'u-owner'
const CHANNEL = '11111111-1111-1111-1111-111111111111'
const DM = '22222222-2222-2222-2222-222222222222'
const assistant = { id: 'agent-row', model: 'aide-personal', legacy: false }
const fleetAgent = { id: 'agent-row-2', model: 'worker-builds', legacy: false }

describe('agent channel reach', () => {
  beforeEach(() => {
    net.queries.length = 0
    net.memberships.clear()
    net.owners.clear()
    net.agentMemberships.clear()
    net.memberships.set(`${CHANNEL}:${OWNER}`, 'member')
    net.memberships.set(`${DM}:${OWNER}`, 'member')
    net.owners.set('aide-personal', OWNER)
  })

  it('a personal assistant lists its owner’s channels, DMs included', async () => {
    const channels = await listChannelsForAgent(assistant)
    expect(channels.map((c) => c.name)).toEqual(['platform', 'dm-with-priya'])
    const dm = channels.find((c) => c.kind === 'dm')
    expect(dm?.peer?.name).toBe('Priya')
    // It took the OWNER-view branch: the membership join asked for the owner,
    // and the agent-membership join never ran.
    expect(net.queries.some((q) => q.includes('join channel_members') && q.includes('join users self'))).toBe(true)
    expect(net.queries.some((q) => q.includes('channel_agents'))).toBe(false)
  })

  it('an ordinary fleet agent still lists only its own memberships', async () => {
    const channels = await listChannelsForAgent(fleetAgent)
    expect(channels).toEqual([]) // not in any channel, and not the owner's view
    expect(net.queries.some((q) => q.includes('channel_agents'))).toBe(true)
  })

  it('a personal assistant may read where its owner is a member', async () => {
    expect(await agentMayAccessChannel(CHANNEL, assistant)).toBe(true)
    expect(await agentMayAccessChannel(DM, assistant)).toBe(true)
    const stranger = '33333333-3333-3333-3333-333333333333'
    expect(await agentMayAccessChannel(stranger, assistant)).toBe(false)
  })

  it('a non-uuid channel id is answered, not thrown', async () => {
    // The reported 500: an empty listing made the model pass the NAME. The
    // uuid column cast throws; the guard must answer false/null instead.
    expect(await agentMayAccessChannel('platform', assistant)).toBe(false)
    expect(await agentMayAccessChannel('#platform', assistant)).toBe(false)
    expect(await channelRole(OWNER, 'platform')).toBeNull()
    expect(net.queries).toEqual([]) // rejected before any query was issued
  })
})

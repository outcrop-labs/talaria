import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import {
  agentPresence,
  agentUnread,
  buildSidebar,
  channelHeading,
  readCollapsed,
  writeCollapsed,
  type SidebarInput,
  type SidebarState,
} from './comms-sidebar'
import type { AgentStatus } from './fleet'

const input = (over: Partial<SidebarInput> = {}): SidebarInput => ({
  channels: [
    { id: 'c-growth', name: 'growth-footage', kind: 'channel', unreadCount: 0 },
    { id: 'c-daily', name: 'daily-focus', kind: 'channel', unreadCount: 3 },
    { id: 'c-ai', name: 'ai', kind: 'channel' },
    { id: 'r-launch', name: 'launch plan', kind: 'group', unreadCount: 0 },
  ],
  people: [
    { id: 'u-gigi', name: 'Gigi', email: 'gigi@paleo.co', dmId: 'dm-gigi', unreadCount: 0 },
    { id: 'u-kari', name: 'Kari', email: 'kari@paleo.co', dmId: 'dm-kari', unreadCount: 2 },
    { id: 'u-chas', name: null, email: 'chas@paleo.co', dmId: null },
  ],
  agents: [
    { id: 'hermes', label: 'Hermes' },
    { id: 'atlas', label: 'Atlas' },
  ],
  threads: [
    { id: 't1', agentModel: 'hermes', title: 'Quarterly numbers', unreadCount: 1 },
    { id: 't2', agentModel: 'hermes', title: 'Gift ideas', unreadCount: 4 },
    { id: 't3', agentModel: 'atlas', title: 'Footage audit', unreadCount: 0 },
  ],
  ...over,
})

const state = (over: Partial<SidebarState> = {}): SidebarState => ({
  query: '',
  collapsed: { channels: false, relays: false, dms: false, starred: false, agents: false },
  active: { channelId: null, agentModel: null },
  ...over,
})

function installLocalStorage() {
  const map = new Map<string, string>()
  vi.stubGlobal('window', {
    localStorage: {
      getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
    },
  })
  return map
}

beforeEach(() => vi.unstubAllGlobals())

// ── search ──────────────────────────────────────────────────────────────────

test('channelHeading writes channels with #, relays with ⇄, and DMs bare', () => {
  assert.equal(channelHeading('channel', 'general'), '#general')
  assert.equal(channelHeading('group', 'launch'), '⇄ launch')
  assert.equal(channelHeading('dm', 'Gigi'), 'Gigi')
})

test('an empty query returns every row, with relays in their own list', () => {
  const v = buildSidebar(input(), state())
  assert.deepEqual(v.channels.map((c) => c.id), ['c-growth', 'c-daily', 'c-ai'])
  assert.deepEqual(v.relays.map((c) => c.id), ['r-launch'])
  assert.deepEqual(v.people.map((p) => p.id), ['u-gigi', 'u-kari', 'u-chas'])
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['hermes', 'atlas'])
  assert.equal(v.searching, false)
  assert.equal(v.noMatches, false)
  // Not searching: no thread list is imposed on the agents.
  assert.ok(v.agents.every((a) => a.matchedThreads === null))
})

test('"gi" finds Gigi, case-insensitively, and nothing that merely shares letters', () => {
  for (const q of ['gi', 'GI', '  Gi  ']) {
    const v = buildSidebar(input(), state({ query: q }))
    assert.deepEqual(v.people.map((p) => p.id), ['u-gigi'], `query ${JSON.stringify(q)}`)
    assert.deepEqual(v.channels, [], 'growth-footage has no "gi" in it')
  }
})

test('"#growth-footage" finds the channel — the # is how you write a channel, not part of its name', () => {
  const v = buildSidebar(input(), state({ query: '#growth-footage' }))
  assert.deepEqual(v.channels.map((c) => c.id), ['c-growth'])
  assert.deepEqual(v.people, [])
  assert.deepEqual(buildSidebar(input(), state({ query: '#GROWTH' })).channels.map((c) => c.id), ['c-growth'])
})

test('people match by email too, and a nameless teammate is still findable', () => {
  const v = buildSidebar(input(), state({ query: 'chas@' }))
  assert.deepEqual(v.people.map((p) => p.id), ['u-chas'])
})

test('a thread-title match keeps its agent, showing only the matching threads', () => {
  // "gift" matches no agent name — only Hermes's thread.
  const v = buildSidebar(input(), state({ query: 'gift' }))
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['hermes'])
  assert.deepEqual(v.agents[0]!.matchedThreads?.map((t) => t.id), ['t2'])
})

test('an agent whose name matches stays, with whatever of its threads also match', () => {
  const v = buildSidebar(input(), state({ query: 'atl' }))
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['atlas'])
  assert.deepEqual(v.agents[0]!.matchedThreads, [])
})

test('a query that hides everything says so', () => {
  const v = buildSidebar(input(), state({ query: 'zzz' }))
  assert.equal(v.searching, true)
  assert.equal(v.noMatches, true)
})

// ── collapse ────────────────────────────────────────────────────────────────

test('a collapsed Channels section still yields the active row and every unread row', () => {
  const v = buildSidebar(
    input(),
    state({ collapsed: { channels: true, relays: false, dms: false, starred: false, agents: false }, active: { channelId: 'c-ai', agentModel: null } }),
  )
  assert.deepEqual(v.channels.map((c) => c.id), ['c-daily', 'c-ai'])
  assert.deepEqual(v.relays.map((c) => c.id), ['r-launch'], 'collapsing Channels leaves Relays alone')
})

test('a collapsed Relays section still yields the active relay and unread relays', () => {
  const quiet = buildSidebar(input(), state({ collapsed: { channels: false, relays: true, dms: false, starred: false, agents: false } }))
  assert.deepEqual(quiet.relays.map((c) => c.id), [], 'read and inactive: hidden')
  assert.equal(quiet.channels.length, 3, 'collapsing Relays leaves Channels alone')
  const active = buildSidebar(
    input(),
    state({ collapsed: { channels: false, relays: true, dms: false, starred: false, agents: false }, active: { channelId: 'r-launch', agentModel: null } }),
  )
  assert.deepEqual(active.relays.map((c) => c.id), ['r-launch'])
})

test('a search matches relays in their own section', () => {
  const v = buildSidebar(input(), state({ query: 'launch' }))
  assert.deepEqual(v.relays.map((c) => c.id), ['r-launch'])
  assert.deepEqual(v.channels.map((c) => c.id), [])
  assert.equal(v.noMatches, false)
})

test('a collapsed Direct messages section keeps the active DM and unread DMs — people only', () => {
  const v = buildSidebar(
    input(),
    state({ collapsed: { channels: false, relays: false, dms: true, starred: false, agents: false }, active: { channelId: 'dm-gigi', agentModel: null } }),
  )
  assert.deepEqual(v.people.map((p) => p.id), ['u-gigi', 'u-kari'])
  // Agents are their own section: collapsing Direct messages leaves them be.
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['hermes', 'atlas'])
})

test('a collapsed Agents section keeps the active agent and agents with unread threads', () => {
  const collapsed = { channels: false, relays: false, dms: false, starred: false, agents: true }
  // Hermes has 5 unread across its threads; Atlas has none and is not active.
  assert.deepEqual(buildSidebar(input(), state({ collapsed })).agents.map((a) => a.agent.id), ['hermes'])
  const active = buildSidebar(input(), state({ collapsed, active: { channelId: null, agentModel: 'atlas' } }))
  assert.deepEqual(active.agents.map((a) => a.agent.id), ['hermes', 'atlas'])
  // People stay listed.
  assert.equal(active.people.length, 3)
})

test('a search looks inside collapsed sections — finding is the point', () => {
  // growth-footage is neither active nor unread, so collapse alone hides it.
  const v = buildSidebar(input(), state({ query: 'growth', collapsed: { channels: true, relays: true, dms: true, starred: false, agents: false } }))
  assert.deepEqual(v.channels.map((c) => c.id), ['c-growth'])
})

test('collapse state persists per section and restores', () => {
  const store = installLocalStorage()
  assert.deepEqual(readCollapsed(), { channels: false, relays: false, dms: false, starred: false, agents: false }, 'nothing stored: all open')
  writeCollapsed({ channels: true, relays: false, dms: false, starred: false, agents: false })
  assert.deepEqual(readCollapsed(), { channels: true, relays: false, dms: false, starred: false, agents: false })
  writeCollapsed({ channels: true, relays: true, dms: true, starred: false, agents: false })
  assert.deepEqual(readCollapsed(), { channels: true, relays: true, dms: true, starred: false, agents: false })
  writeCollapsed({ channels: false, relays: false, dms: false, starred: false, agents: true })
  assert.equal(readCollapsed().agents, true)
  // A value stored before Relays (or Agents) had its own section reads it as open.
  store.set('talaria:comms-sidebar-collapsed', '{"channels":true,"dms":false}')
  assert.deepEqual(readCollapsed(), { channels: true, relays: false, dms: false, starred: false, agents: false })
})

test('a malformed collapse value degrades to all open', () => {
  const store = installLocalStorage()
  store.set('talaria:comms-sidebar-collapsed', '"nope"')
  assert.deepEqual(readCollapsed(), { channels: false, relays: false, dms: false, starred: false, agents: false })
  store.set('talaria:comms-sidebar-collapsed', '{"channels":"yes"}')
  assert.deepEqual(readCollapsed(), { channels: false, relays: false, dms: false, starred: false, agents: false })
})

// ── starred ─────────────────────────────────────────────────────────────────

// Gigi's DM, the launch relay, Hermes, and the growth channel — in star order.
const STARS = ['channel:dm-gigi', 'channel:r-launch', 'agent:hermes', 'channel:c-growth']
const dmChannel = { id: 'dm-gigi', name: 'Gigi', kind: 'dm' as const, unreadCount: 0 }
const withDm = () => input({ channels: [...input().channels, dmChannel], stars: STARS })
const starredIds = (v: ReturnType<typeof buildSidebar>) =>
  v.starred.map((s) =>
    s.kind === 'channel' ? s.channel.id : s.kind === 'person' ? s.person.id : s.kind === 'thread' ? s.thread.id : s.row.agent.id,
  )

test('nothing starred: an empty Starred section and every row where it was', () => {
  const v = buildSidebar(input(), state())
  assert.deepEqual(v.starred, [])
  assert.deepEqual(buildSidebar(input({ stars: [] }), state()).starred, [])
  assert.equal(buildSidebar(input({ stars: ['channel:gone'] }), state()).starredTotal, 0)
})

test('starred rows leave their own section and list in Starred, in star order', () => {
  const v = buildSidebar(withDm(), state())
  assert.deepEqual(starredIds(v), ['u-gigi', 'r-launch', 'hermes', 'c-growth'])
  assert.deepEqual(
    v.starred.map((s) => s.kind),
    ['person', 'channel', 'agent', 'channel'],
  )
  assert.deepEqual(v.channels.map((c) => c.id), ['c-daily', 'c-ai'])
  assert.deepEqual(v.relays, [])
  assert.deepEqual(v.people.map((p) => p.id), ['u-kari', 'u-chas'])
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['atlas'])
  // A starred agent row carries its unread like any agent row.
  const hermes = v.starred.find((s) => s.kind === 'agent')
  assert.ok(hermes?.kind === 'agent')
  assert.equal(hermes.row.unread, 5)
  assert.equal(hermes.row.matchedThreads, null)
})

test('a star whose conversation is gone is simply not shown', () => {
  const v = buildSidebar(
    input({
      // dm-ghost is a DM whose peer is not in the directory; dm-gigi is not loaded at all.
      channels: [...input().channels, { id: 'dm-ghost', name: 'Ghost', kind: 'dm' }],
      stars: ['channel:gone', 'agent:retired', 'channel:dm-ghost', 'channel:dm-gigi', 'channel:c-ai'],
    }),
    state(),
  )
  assert.deepEqual(starredIds(v), ['c-ai'])
  // Gigi's DM isn't loaded, so her row stays under Direct messages.
  assert.deepEqual(v.people.map((p) => p.id), ['u-gigi', 'u-kari', 'u-chas'])
})

test('unknown star kinds are ignored', () => {
  const v = buildSidebar(input({ stars: ['board:b1', 'c-ai', 'agent:atlas'] }), state())
  assert.deepEqual(starredIds(v), ['atlas'])
  assert.deepEqual(v.channels.map((c) => c.id), ['c-growth', 'c-daily', 'c-ai'])
})

test('a collapsed Starred section still yields the active row and every unread row', () => {
  const collapsed = { channels: false, relays: false, dms: false, starred: true, agents: false }
  const quiet = buildSidebar(withDm(), state({ collapsed }))
  // Only Hermes has unread (5, across its threads).
  assert.deepEqual(starredIds(quiet), ['hermes'])
  const activeChannel = buildSidebar(withDm(), state({ collapsed, active: { channelId: 'r-launch', agentModel: null } }))
  assert.deepEqual(starredIds(activeChannel), ['r-launch', 'hermes'])
  const activeDm = buildSidebar(withDm(), state({ collapsed, active: { channelId: 'dm-gigi', agentModel: null } }))
  assert.deepEqual(starredIds(activeDm), ['u-gigi', 'hermes'])
  const unreadChannel = buildSidebar(
    input({ stars: ['channel:c-daily', 'agent:atlas'] }),
    state({ collapsed, active: { channelId: null, agentModel: 'atlas' } }),
  )
  assert.deepEqual(starredIds(unreadChannel), ['c-daily', 'atlas'])
  // Collapsing Starred never puts a starred row back in its own section.
  assert.deepEqual(quiet.channels.map((c) => c.id), ['c-daily', 'c-ai'])
  // The header still knows there are stars to unfold.
  assert.equal(quiet.starredTotal, 4)
})

test('a search filters starred rows too, and looks inside a collapsed Starred', () => {
  const collapsed = { channels: false, relays: false, dms: false, starred: true, agents: false }
  const gi = buildSidebar(withDm(), state({ query: 'gigi', collapsed }))
  assert.deepEqual(starredIds(gi), ['u-gigi'])
  assert.deepEqual(gi.people, [], 'Gigi is in Starred, not Direct messages')
  const growth = buildSidebar(withDm(), state({ query: '#growth' }))
  assert.deepEqual(starredIds(growth), ['c-growth'])
  assert.deepEqual(growth.channels, [])
  // A starred agent found by a thread title shows just the matching threads.
  const gift = buildSidebar(withDm(), state({ query: 'gift' }))
  assert.deepEqual(starredIds(gift), ['hermes'])
  const row = gift.starred[0]
  assert.ok(row?.kind === 'agent')
  assert.deepEqual(row.row.matchedThreads?.map((t) => t.id), ['t2'])
})

test('noMatches counts starred rows: a query found only in Starred is a match', () => {
  const v = buildSidebar(withDm(), state({ query: 'launch' }))
  assert.deepEqual(starredIds(v), ['r-launch'])
  assert.deepEqual(v.relays, [])
  assert.equal(v.noMatches, false)
  assert.equal(buildSidebar(withDm(), state({ query: 'zzz' })).noMatches, true)
})

test('Starred collapse persists with the others, and an older stored value reads it as open', () => {
  const store = installLocalStorage()
  writeCollapsed({ channels: false, relays: false, dms: false, starred: true, agents: false })
  assert.deepEqual(readCollapsed(), { channels: false, relays: false, dms: false, starred: true, agents: false })
  store.set('talaria:comms-sidebar-collapsed', '{"channels":true,"relays":true,"dms":false}')
  assert.deepEqual(readCollapsed(), { channels: true, relays: true, dms: false, starred: false, agents: false })
  store.set('talaria:comms-sidebar-collapsed', '{"starred":"yes"}')
  assert.deepEqual(readCollapsed(), { channels: false, relays: false, dms: false, starred: false, agents: false })
})

// ── unread + presence ───────────────────────────────────────────────────────

test("an agent's unread is the sum across its conversations", () => {
  const threads = input().threads
  assert.equal(agentUnread('hermes', threads), 5)
  assert.equal(agentUnread('atlas', threads), 0)
  assert.equal(agentUnread('nobody', threads), 0)
  const v = buildSidebar(input(), state())
  assert.deepEqual(v.agents.map((a) => [a.agent.id, a.unread]), [['hermes', 5], ['atlas', 0]])
})

test('fleet status maps to presence: offline is offline, everything else is online', () => {
  const expected: Record<AgentStatus, 'online' | 'offline'> = {
    offline: 'offline',
    idle: 'online',
    busy: 'online',
    error: 'online',
  }
  for (const [status, presence] of Object.entries(expected)) {
    assert.equal(agentPresence(status as AgentStatus), presence, status)
  }
  // No fleet row (not loaded, or not reporting) is not evidence of life.
  assert.equal(agentPresence(undefined), 'offline')
})

// ── pinned agent threads ────────────────────────────────────────────────────

test('a pinned thread lists in Starred, with its agent, and leaves its agent', () => {
  const v = buildSidebar(input({ stars: ['thread:t2', 'thread:gone'] }), state())
  assert.deepEqual(starredIds(v), ['t2'])
  const pinned = v.starred[0]
  assert.ok(pinned?.kind === 'thread')
  assert.equal(pinned.agent.id, 'hermes')
  assert.deepEqual([...v.starredThreadIds], ['t2'])
  // The agent itself stays in Direct messages; only the thread moved.
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['hermes', 'atlas'])
})

test('a search matches a pinned thread by title or by its agent, never under the agent too', () => {
  const byTitle = buildSidebar(input({ stars: ['thread:t2'] }), state({ query: 'gift' }))
  assert.deepEqual(starredIds(byTitle), ['t2'])
  assert.deepEqual(byTitle.agents, [], 'Hermes has no other thread matching "gift"')
  const byAgent = buildSidebar(input({ stars: ['thread:t2'] }), state({ query: 'hermes' }))
  assert.deepEqual(starredIds(byAgent), ['t2'])
})

test('a collapsed Starred keeps a pinned thread when it is open or unread', () => {
  const collapsed = { channels: false, relays: false, dms: false, starred: true, agents: false }
  // t3 is read; t2 has 4 unread.
  const quiet = buildSidebar(input({ stars: ['thread:t3', 'thread:t2'] }), state({ collapsed }))
  assert.deepEqual(starredIds(quiet), ['t2'])
  const open = buildSidebar(
    input({ stars: ['thread:t3', 'thread:t2'] }),
    state({ collapsed, active: { channelId: null, agentModel: 'atlas', threadId: 't3' } }),
  )
  assert.deepEqual(starredIds(open), ['t3', 't2'])
})

// ── group DMs ───────────────────────────────────────────────────────────────

const groupDm = { id: 'g-launch', name: '', kind: 'dm' as const, unreadCount: 0, group: true, label: 'Gigi, Kari, Hermes' }
const withGroup = (over: Partial<SidebarInput> = {}) =>
  input({ channels: [...input().channels, { id: 'dm-gigi', name: 'Gigi', kind: 'dm' }, groupDm], ...over })

test('a group DM lists after the people, and a person’s own DM does not', () => {
  const v = buildSidebar(withGroup(), state())
  assert.deepEqual(v.groupDms.map((c) => c.id), ['g-launch'])
  assert.deepEqual(v.people.map((p) => p.id), ['u-gigi', 'u-kari', 'u-chas'])
})

test('a search finds a group DM by its label', () => {
  assert.deepEqual(buildSidebar(withGroup(), state({ query: 'kari, herm' })).groupDms.map((c) => c.id), ['g-launch'])
  assert.deepEqual(buildSidebar(withGroup(), state({ query: 'zzz' })).groupDms, [])
})

test('a starred group DM moves to Starred as itself', () => {
  const v = buildSidebar(withGroup({ stars: ['channel:g-launch'] }), state())
  assert.deepEqual(v.groupDms, [])
  const s = v.starred[0]
  assert.ok(s?.kind === 'channel')
  assert.equal(s.channel.id, 'g-launch')
})

test('a collapsed Direct messages keeps the active and unread group DMs', () => {
  const collapsed = { channels: false, relays: false, dms: true, starred: false, agents: false }
  assert.deepEqual(buildSidebar(withGroup(), state({ collapsed })).groupDms, [])
  const active = buildSidebar(withGroup(), state({ collapsed, active: { channelId: 'g-launch', agentModel: null } }))
  assert.deepEqual(active.groupDms.map((c) => c.id), ['g-launch'])
})

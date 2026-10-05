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
  collapsed: { channels: false, dms: false },
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

test('an empty query returns every row, channels before relays', () => {
  const v = buildSidebar(input(), state())
  assert.deepEqual(v.channels.map((c) => c.id), ['c-growth', 'c-daily', 'c-ai', 'r-launch'])
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
    state({ collapsed: { channels: true, dms: false }, active: { channelId: 'c-ai', agentModel: null } }),
  )
  assert.deepEqual(v.channels.map((c) => c.id), ['c-daily', 'c-ai'])
})

test('a collapsed Direct messages section keeps the active DM, unread DMs, and agents with unread threads', () => {
  const v = buildSidebar(
    input(),
    state({ collapsed: { channels: false, dms: true }, active: { channelId: 'dm-gigi', agentModel: null } }),
  )
  assert.deepEqual(v.people.map((p) => p.id), ['u-gigi', 'u-kari'])
  // Hermes has 5 unread across its threads; Atlas has none and is not active.
  assert.deepEqual(v.agents.map((a) => a.agent.id), ['hermes'])
  const active = buildSidebar(input(), state({ collapsed: { channels: false, dms: true }, active: { channelId: null, agentModel: 'atlas' } }))
  assert.deepEqual(active.agents.map((a) => a.agent.id), ['hermes', 'atlas'])
})

test('a search looks inside collapsed sections — finding is the point', () => {
  // growth-footage is neither active nor unread, so collapse alone hides it.
  const v = buildSidebar(input(), state({ query: 'growth', collapsed: { channels: true, dms: true } }))
  assert.deepEqual(v.channels.map((c) => c.id), ['c-growth'])
})

test('collapse state persists per section and restores', () => {
  installLocalStorage()
  assert.deepEqual(readCollapsed(), { channels: false, dms: false }, 'nothing stored: both open')
  writeCollapsed({ channels: true, dms: false })
  assert.deepEqual(readCollapsed(), { channels: true, dms: false })
  writeCollapsed({ channels: true, dms: true })
  assert.deepEqual(readCollapsed(), { channels: true, dms: true })
})

test('a malformed collapse value degrades to both open', () => {
  const store = installLocalStorage()
  store.set('talaria:comms-sidebar-collapsed', '"nope"')
  assert.deepEqual(readCollapsed(), { channels: false, dms: false })
  store.set('talaria:comms-sidebar-collapsed', '{"channels":"yes"}')
  assert.deepEqual(readCollapsed(), { channels: false, dms: false })
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

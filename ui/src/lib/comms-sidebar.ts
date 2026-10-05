// THE COMMS SIDEBAR'S ROW LOGIC, pure (KTD9).
//
// The rail is: Find a conversation → Threads → Drafts & sent → divider →
// Channels (channels, then relays) → Direct messages (people, then agents with
// their threads nested). Everything that decides WHICH rows show lives here so
// it can be tested without a DOM:
//
//   • SEARCH is a client-side, case-insensitive substring filter over loaded
//     rows. A leading `#` is how a channel is written, not part of its name, so
//     it is stripped. People match by name or email. Agents match by label OR
//     by any of their thread titles — a thread match keeps its agent and shows
//     just the matching threads under it.
//   • COLLAPSE hides a section's rows except the active one and every unread
//     one (Slack's behavior: collapsing never hides what needs you). A search
//     looks inside collapsed sections — finding is the point of a search.
//   • An agent's UNREAD is the sum across its conversations.
//   • Agent PRESENCE derives from fleet status (KTD6): offline is offline,
//     every other status means the agent is up.

import { readStored, writeStored } from './persist'
import type { AgentStatus } from './fleet'

export type Presence = 'online' | 'offline'

export interface SidebarChannel {
  id: string
  name: string
  kind: 'channel' | 'group' | 'dm'
  unreadCount?: number
}

export interface SidebarPerson {
  id: string
  name: string | null
  email: string | null
  /** Their DM channel with you, when one exists. */
  dmId: string | null
  unreadCount?: number
}

export interface SidebarAgent {
  id: string
  label: string
}

export interface SidebarThread {
  id: string
  agentModel: string
  title: string | null
  unreadCount?: number
}

export interface SidebarInput<
  C extends SidebarChannel = SidebarChannel,
  P extends SidebarPerson = SidebarPerson,
  A extends SidebarAgent = SidebarAgent,
  T extends SidebarThread = SidebarThread,
> {
  channels: C[]
  people: P[]
  agents: A[]
  threads: T[]
}

export interface Collapsed {
  channels: boolean
  dms: boolean
}

export interface SidebarState {
  query: string
  collapsed: Collapsed
  /** The selected channel/DM id, or the selected agent's model. */
  active: { channelId: string | null; agentModel: string | null }
}

export interface AgentRow<A extends SidebarAgent = SidebarAgent, T extends SidebarThread = SidebarThread> {
  agent: A
  unread: number
  /** While searching: the threads that matched (shown expanded, no cap).
   *  Null when not searching — the rail's usual expansion rules apply. */
  matchedThreads: T[] | null
}

export interface SidebarView<
  C extends SidebarChannel = SidebarChannel,
  P extends SidebarPerson = SidebarPerson,
  A extends SidebarAgent = SidebarAgent,
  T extends SidebarThread = SidebarThread,
> {
  /** Channels first, then relays. */
  channels: C[]
  people: P[]
  agents: AgentRow<A, T>[]
  searching: boolean
  /** A query is active and it hid every row in every section. */
  noMatches: boolean
}

/** Lowercased, trimmed, and without the `#` people type before a channel. */
export function normalizeQuery(query: string): string {
  return query.trim().replace(/^#+/, '').trim().toLowerCase()
}

const hit = (q: string, ...fields: (string | null | undefined)[]): boolean =>
  fields.some((f) => !!f && f.toLowerCase().includes(q))

export function agentUnread(model: string, threads: SidebarThread[]): number {
  let n = 0
  for (const t of threads) if (t.agentModel === model) n += t.unreadCount ?? 0
  return n
}

/** How a channel is written in a heading: `#name` for a channel, `⇄ name` for
 *  a relay (group), the bare name otherwise — a DM's name is the caller's. */
export function channelHeading(kind: SidebarChannel['kind'], name: string): string {
  if (kind === 'channel') return `#${name}`
  if (kind === 'group') return `⇄ ${name}`
  return name
}

/** Fleet status → the presence dot. No fleet row is no evidence of life. */
export function agentPresence(status: AgentStatus | undefined): Presence {
  return status === undefined || status === 'offline' ? 'offline' : 'online'
}

export function buildSidebar<
  C extends SidebarChannel,
  P extends SidebarPerson,
  A extends SidebarAgent,
  T extends SidebarThread,
>(input: SidebarInput<C, P, A, T>, state: SidebarState): SidebarView<C, P, A, T> {
  const q = normalizeQuery(state.query)
  const searching = q.length > 0
  const { channelId, agentModel } = state.active

  const rooms = input.channels.filter((c) => c.kind === 'channel')
  const relays = input.channels.filter((c) => c.kind === 'group')
  let channels = [...rooms, ...relays]
  let people = input.people
  let agents: AgentRow<A, T>[] = input.agents.map((agent) => ({
    agent,
    unread: agentUnread(agent.id, input.threads),
    matchedThreads: null,
  }))

  if (searching) {
    channels = channels.filter((c) => hit(q, c.name))
    people = people.filter((p) => hit(q, p.name, p.email))
    agents = agents
      .map((row) => {
        const matched = input.threads.filter((t) => t.agentModel === row.agent.id && hit(q, t.title))
        return { ...row, matchedThreads: matched }
      })
      .filter((row) => hit(q, row.agent.label) || row.matchedThreads.length > 0)
  } else {
    if (state.collapsed.channels) {
      channels = channels.filter((c) => c.id === channelId || (c.unreadCount ?? 0) > 0)
    }
    if (state.collapsed.dms) {
      people = people.filter((p) => (p.dmId !== null && p.dmId === channelId) || (p.unreadCount ?? 0) > 0)
      agents = agents.filter((row) => row.agent.id === agentModel || row.unread > 0)
    }
  }

  return {
    channels,
    people,
    agents,
    searching,
    noMatches: searching && channels.length === 0 && people.length === 0 && agents.length === 0,
  }
}

// ── collapse persistence ────────────────────────────────────────────────────

const COLLAPSED_KEY = 'talaria:comms-sidebar-collapsed'
const OPEN: Collapsed = { channels: false, dms: false }

function parseCollapsed(raw: unknown): Collapsed | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const v = raw as Record<string, unknown>
  if ((v.channels !== undefined && typeof v.channels !== 'boolean') || (v.dms !== undefined && typeof v.dms !== 'boolean')) {
    return null
  }
  return { channels: v.channels === true, dms: v.dms === true }
}

export function readCollapsed(): Collapsed {
  return readStored(COLLAPSED_KEY, parseCollapsed, OPEN)
}

export function writeCollapsed(c: Collapsed): void {
  writeStored(COLLAPSED_KEY, { channels: c.channels, dms: c.dms })
}

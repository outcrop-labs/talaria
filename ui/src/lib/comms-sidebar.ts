// THE COMMS SIDEBAR'S ROW LOGIC, pure (KTD9).
//
// The rail is: Find a conversation → Threads → Drafts & sent → divider →
// Starred → Channels → Relays → Direct messages (people) → Agents (each with
// its threads nested). Everything that decides WHICH rows show lives here so
// it can be tested without a DOM:
//
//   • SEARCH is a client-side, case-insensitive substring filter over loaded
//     rows. A leading `#` is how a channel is written, not part of its name, so
//     it is stripped. People match by name or email. Agents match by label OR
//     by any of their thread titles — a thread match keeps its agent and shows
//     just the matching threads under it.
//   • COLLAPSE hides a section's rows except the active one and every unread
//     one (collapsing never hides what needs you). A search
//     looks inside collapsed sections — finding is the point of a search.
//   • STARRED rows (lib/comms-stars.ts) leave their own section and list in
//     Starred instead, in star order — channels, relays, people (by their DM)
//     agents and pinned agent threads side by side. A pinned thread leaves its
//     agent's nested list. A star whose conversation is gone shows nothing;
//     the same search and collapse rules apply there as elsewhere.
//   • An agent's UNREAD is the sum across its conversations.
//   • Agent PRESENCE derives from fleet status (KTD6): offline is offline,
//     every other status means the agent is up.

import { readStored, writeStored } from './persist'
import type { AgentStatus } from './fleet'
import { parseStarKey } from './comms-stars'

export type Presence = 'online' | 'offline'

export interface SidebarChannel {
  id: string
  name: string
  kind: 'channel' | 'group' | 'dm'
  unreadCount?: number
  /** A DM of several people and/or agents (comms-dm.ts `isGroupDm`). */
  group?: boolean
  /** What a group DM is called on its row (comms-dm.ts) — search matches it. */
  label?: string
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
  /** Star keys (`channel:<id>` / `agent:<model>` / `thread:<id>`), in star order. */
  stars?: readonly string[]
}

export interface Collapsed {
  channels: boolean
  relays: boolean
  dms: boolean
  starred: boolean
  agents: boolean
}

export interface SidebarState {
  query: string
  collapsed: Collapsed
  /** The selected channel/DM id, or the selected agent's model (and thread). */
  active: { channelId: string | null; agentModel: string | null; threadId?: string | null }
}

export interface AgentRow<A extends SidebarAgent = SidebarAgent, T extends SidebarThread = SidebarThread> {
  agent: A
  unread: number
  /** While searching: the threads that matched (shown expanded, no cap).
   *  Null when not searching — the rail's usual expansion rules apply. */
  matchedThreads: T[] | null
}

/** One row of the Starred section — whichever kind of row it was starred from. */
export type StarredItem<
  C extends SidebarChannel = SidebarChannel,
  P extends SidebarPerson = SidebarPerson,
  A extends SidebarAgent = SidebarAgent,
  T extends SidebarThread = SidebarThread,
> =
  | { kind: 'channel'; key: string; channel: C }
  | { kind: 'person'; key: string; person: P }
  | { kind: 'agent'; key: string; row: AgentRow<A, T> }
  | { kind: 'thread'; key: string; thread: T; agent: A }

export interface SidebarView<
  C extends SidebarChannel = SidebarChannel,
  P extends SidebarPerson = SidebarPerson,
  A extends SidebarAgent = SidebarAgent,
  T extends SidebarThread = SidebarThread,
> {
  /** Starred rows of every kind, in star order — absent from their own section. */
  starred: StarredItem<C, P, A, T>[]
  /** How many stars resolved to a row, before search and collapse — whether
   *  the Starred section exists at all. */
  starredTotal: number
  /** Ids of pinned threads — listed in Starred, so not under their agent. */
  starredThreadIds: ReadonlySet<string>
  /** `#` channels only. */
  channels: C[]
  /** Relays (kind `group`), their own section. */
  relays: C[]
  people: P[]
  /** Group DMs (several people and/or agents), after the people. */
  groupDms: C[]
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
  const threadId = state.active.threadId ?? null

  const agentRow = (agent: A): AgentRow<A, T> => ({
    agent,
    unread: agentUnread(agent.id, input.threads),
    matchedThreads: null,
  })

  // Resolve the stars to rows. A star is shown at most once, and only when
  // its target is loaded: a channel/relay by id, a DM through its peer.
  let starred: StarredItem<C, P, A, T>[] = []
  const starredChannels = new Set<string>()
  const starredAgents = new Set<string>()
  const starredThreadIds = new Set<string>()
  for (const key of input.stars ?? []) {
    const star = parseStarKey(key)
    if (!star) continue
    if (star.kind === 'thread') {
      const thread = input.threads.find((t) => t.id === star.id)
      const agent = thread && input.agents.find((a) => a.id === thread.agentModel)
      if (!thread || !agent || starredThreadIds.has(thread.id)) continue
      starredThreadIds.add(thread.id)
      starred.push({ kind: 'thread', key, thread, agent })
      continue
    }
    if (star.kind === 'agent') {
      const agent = input.agents.find((a) => a.id === star.id)
      if (!agent || starredAgents.has(agent.id)) continue
      starredAgents.add(agent.id)
      starred.push({ kind: 'agent', key, row: agentRow(agent) })
      continue
    }
    const channel = input.channels.find((c) => c.id === star.id)
    if (!channel || starredChannels.has(channel.id)) continue
    if (channel.kind === 'dm') {
      // A group DM stars as itself; a two-person DM as its person, and not at
      // all while that person isn't in the directory.
      if (channel.group) {
        starredChannels.add(channel.id)
        starred.push({ kind: 'channel', key, channel })
        continue
      }
      const person = input.people.find((p) => p.dmId === channel.id)
      if (!person) continue
      starredChannels.add(channel.id)
      starred.push({ kind: 'person', key, person })
    } else {
      starredChannels.add(channel.id)
      starred.push({ kind: 'channel', key, channel })
    }
  }

  const starredTotal = starred.length

  let channels = input.channels.filter((c) => c.kind === 'channel' && !starredChannels.has(c.id))
  let relays = input.channels.filter((c) => c.kind === 'group' && !starredChannels.has(c.id))
  let people = input.people.filter((p) => p.dmId === null || !starredChannels.has(p.dmId))
  let groupDms = input.channels.filter((c) => c.kind === 'dm' && c.group && !starredChannels.has(c.id))
  let agents: AgentRow<A, T>[] = input.agents.filter((a) => !starredAgents.has(a.id)).map(agentRow)

  if (searching) {
    const matchAgent = (row: AgentRow<A, T>): AgentRow<A, T> | null => {
      const matched = input.threads.filter(
        (t) => t.agentModel === row.agent.id && !starredThreadIds.has(t.id) && hit(q, t.title),
      )
      return hit(q, row.agent.label) || matched.length > 0 ? { ...row, matchedThreads: matched } : null
    }
    channels = channels.filter((c) => hit(q, c.name))
    relays = relays.filter((c) => hit(q, c.name))
    people = people.filter((p) => hit(q, p.name, p.email))
    groupDms = groupDms.filter((c) => hit(q, c.label, c.name))
    agents = agents.map(matchAgent).filter((row): row is AgentRow<A, T> => row !== null)
    starred = starred.flatMap((s): StarredItem<C, P, A, T>[] => {
      if (s.kind === 'channel') return hit(q, s.channel.name, s.channel.label) ? [s] : []
      if (s.kind === 'person') return hit(q, s.person.name, s.person.email) ? [s] : []
      if (s.kind === 'thread') return hit(q, s.thread.title, s.agent.label) ? [s] : []
      const row = matchAgent(s.row)
      return row ? [{ ...s, row }] : []
    })
  } else {
    // A collapsed section still shows the active row and every unread one.
    const keep = (c: C) => c.id === channelId || (c.unreadCount ?? 0) > 0
    const keepPerson = (p: P) => (p.dmId !== null && p.dmId === channelId) || (p.unreadCount ?? 0) > 0
    const keepAgent = (row: AgentRow<A, T>) => row.agent.id === agentModel || row.unread > 0
    const keepThread = (t: T) => t.id === threadId || (t.unreadCount ?? 0) > 0
    if (state.collapsed.starred) {
      starred = starred.filter((s) =>
        s.kind === 'channel'
          ? keep(s.channel)
          : s.kind === 'person'
            ? keepPerson(s.person)
            : s.kind === 'thread'
              ? keepThread(s.thread)
              : keepAgent(s.row),
      )
    }
    if (state.collapsed.channels) channels = channels.filter(keep)
    if (state.collapsed.relays) relays = relays.filter(keep)
    if (state.collapsed.dms) {
      people = people.filter(keepPerson)
      groupDms = groupDms.filter(keep)
    }
    if (state.collapsed.agents) agents = agents.filter(keepAgent)
  }

  return {
    starred,
    starredTotal,
    starredThreadIds,
    channels,
    relays,
    people,
    groupDms,
    agents,
    searching,
    noMatches:
      searching &&
      starred.length === 0 &&
      channels.length === 0 &&
      relays.length === 0 &&
      people.length === 0 &&
      groupDms.length === 0 &&
      agents.length === 0,
  }
}

// ── collapse persistence ────────────────────────────────────────────────────

const COLLAPSED_KEY = 'talaria:comms-sidebar-collapsed'
const OPEN: Collapsed = { channels: false, relays: false, dms: false, starred: false, agents: false }
const SECTIONS = ['channels', 'relays', 'dms', 'starred', 'agents'] as const

function parseCollapsed(raw: unknown): Collapsed | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const v = raw as Record<string, unknown>
  // A key stored before its section existed is simply absent: read it as open.
  for (const k of SECTIONS) {
    if (v[k] !== undefined && typeof v[k] !== 'boolean') return null
  }
  return {
    channels: v.channels === true,
    relays: v.relays === true,
    dms: v.dms === true,
    starred: v.starred === true,
    agents: v.agents === true,
  }
}

export function readCollapsed(): Collapsed {
  return readStored(COLLAPSED_KEY, parseCollapsed, OPEN)
}

export function writeCollapsed(c: Collapsed): void {
  writeStored(COLLAPSED_KEY, { channels: c.channels, relays: c.relays, dms: c.dms, starred: c.starred, agents: c.agents })
}

// THE COMMS PROFILE DRAWER'S PURE HALF: who the drawer is about, what their
// clock reads, and how a shared conversation row is drawn and where it goes.
// The drawer (routes/app/comms/ProfileDrawer.svelte) and the all-conversations
// view (ConversationsWithView.svelte) render these; the reads are in
// comms-api.ts (`useConversationsWith`).

import type { CommsSelection } from './comms-selection'
import type { Presence } from './comms-sidebar'

/** Who a profile is about: a person (by directory id) or an agent (by model). */
export type ProfileSubject = { kind: 'person'; userId: string } | { kind: 'agent'; model: string }

/** One row of GET /api/users/{id}/conversations or /api/agents/{id}/conversations. */
export interface SharedConversation {
  id: string
  /** The channel kind, or 'agent' for an agent-DM thread. */
  kind: 'dm' | 'channel' | 'group' | 'agent'
  /** The other person's name for a DM; the thread title for an agent thread. */
  name: string
  lastAt: string
  unreadCount: number
}

/**
 * "2:12 PM local time" on THEIR clock, or null when they have no zone set (or
 * one this runtime cannot resolve) — no time is better than a wrong one.
 */
export function localTimeLabel(
  timeZone: string | null | undefined,
  now: Date = new Date(),
  locale?: string,
): string | null {
  if (!timeZone) return null
  try {
    const time = new Intl.DateTimeFormat(locale, { timeZone, hour: 'numeric', minute: '2-digit' }).format(now)
    return `${time} local time`
  } catch {
    return null
  }
}

/** The name a sentence uses: the first word of the display name, else the
 *  email's local part, else "them". */
export function firstName(name: string | null | undefined, email: string | null | undefined): string {
  const word = (name ?? '').trim().split(/\s+/)[0]
  if (word) return word
  const local = (email ?? '').split('@')[0]
  return local || 'them'
}

/** A room's glyph — `#` channel, `⇄` relay — or null where the row takes an
 *  avatar instead (a DM, an agent thread). */
export function conversationGlyph(kind: SharedConversation['kind']): '#' | '⇄' | null {
  if (kind === 'channel') return '#'
  if (kind === 'group') return '⇄'
  return null
}

/** Where clicking the row goes. An agent thread opens under its agent, so it
 *  needs the agent's model; without one it has nowhere to go. */
export function conversationSelection(c: SharedConversation, agentModel: string | null): CommsSelection | null {
  if (c.kind !== 'agent') return { t: 'channel', id: c.id }
  return agentModel ? { t: 'agent', model: agentModel, conversationId: c.id } : null
}

export function presenceLabel(p: Presence): 'Active' | 'Away' {
  return p === 'online' ? 'Active' : 'Away'
}

/** The all-conversations view's path: /comms/with/<person|agent>/<id>. */
export function profileWithPath(s: ProfileSubject): string {
  return s.kind === 'person'
    ? `/comms/with/person/${encodeURIComponent(s.userId)}`
    : `/comms/with/agent/${encodeURIComponent(s.model)}`
}

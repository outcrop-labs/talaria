// Shared message shape for <ChatView> and its turn rows (see ChatView.svelte).
import type { ToolCall } from '@/lib/sse-parse'
import type { MessageReaction, StoredMessage } from '@/lib/conversations.svelte'
import type { ReactionChip } from './ReactionChips.svelte'
import type { Attachment } from '@/lib/attachments'
import type { GuardFinding } from '@/components/chat/guard-caveat'
import type { ChatChip } from '@/lib/chips'
import { zoneOrUndefined } from '@/lib/day-dividers'

export interface DisplayMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning?: string
  tools?: ToolCall[]
  status?: 'streaming' | 'complete' | 'error'
  /** The persisted row's seq, when the row came from the server — synthetic
   *  streaming rows have none. The read cursor advances off this. */
  seq?: number
  attachments?: Attachment[]
  /** Who wrote a user turn — shown in multiplayer plans to tell voices apart. */
  authorLabel?: string | null
  /** Who wrote a user turn, by user id — decides whose photo it shows. */
  authorUserId?: string | null
  /** Confab-guard findings pinned to a reply (annotate/strict modes). */
  guard?: GuardFinding[] | null
  /** The server auto-resumed this turn after its stream died mid-flight —
   *  shown as a marker on the row, never as a second turn. */
  resumed?: boolean
  chips?: ChatChip[]
  /** The persisted row's id — absent on synthetic rows until a sync stamps
   *  it. No id → no action bar (there is nothing to react to yet). */
  id?: string
  /** ISO send time. Synthetic rows get the client's clock so the day
   *  divider places them; the sync replaces it with the server's. */
  createdAt?: string
  reactions?: MessageReaction[]
}

/** Who is looking — a user reaction's actor may be their id or their email
 *  (channels store the email), so `mine` checks both. */
export interface Viewer {
  id?: string | null
  email?: string | null
}

export const toDisplay = (m: StoredMessage): DisplayMessage => ({
  role: m.role,
  content: m.content,
  reasoning: m.reasoning,
  tools: m.tools,
  status: m.status,
  seq: m.seq,
  attachments: m.attachments,
  authorLabel: m.authorLabel,
  guard: m.guard,
  resumed: m.metadata?.resumed === true,
  chips: m.chips,
  authorUserId: m.authorUserId,
  id: m.id,
  createdAt: m.createdAt,
  reactions: m.reactions,
})

const isMe = (viewer: Viewer, actor: string, type: string | undefined): boolean =>
  (type ?? 'user') === 'user' && (actor === viewer.id || actor === viewer.email)

/** Wire reactions → the chips ReactionChips draws: count, the viewer's own
 *  highlighted, and the reactors named on hover. */
export function reactionChips(
  reactions: readonly MessageReaction[] | undefined,
  viewer: Viewer,
  agentLabel: (actor: string) => string = (a) => a,
): ReactionChip[] {
  return (reactions ?? []).map((r) => ({
    emoji: r.emoji,
    count: r.actors.length,
    mine: r.actors.some((a, i) => isMe(viewer, a, r.actorTypes[i])),
    title: r.actors
      .map((a, i) => {
        const type = r.actorTypes[i] ?? 'user'
        if (type !== 'user') return agentLabel(a)
        if (isMe(viewer, a, type)) return 'You'
        return a.split('@')[0] || a
      })
      .join(', '),
  }))
}

/** The optimistic half of a toggle: remove the viewer's reaction if they
 *  have one on this emoji, else add it (as their email, the channel
 *  convention, falling back to their id). An emptied emoji drops out. */
export function toggleReactionLocal(
  reactions: readonly MessageReaction[] | undefined,
  emoji: string,
  viewer: Viewer,
): MessageReaction[] {
  const list = reactions ?? []
  const hit = list.find((r) => r.emoji === emoji)
  const mineAt = hit ? hit.actors.findIndex((a, i) => isMe(viewer, a, hit.actorTypes[i])) : -1
  if (hit && mineAt >= 0) {
    const actors = hit.actors.filter((_, i) => i !== mineAt)
    const actorTypes = hit.actorTypes.filter((_, i) => i !== mineAt)
    return actors.length === 0
      ? list.filter((r) => r !== hit)
      : list.map((r) => (r === hit ? { emoji, actors, actorTypes } : r))
  }
  const me = viewer.email ?? viewer.id ?? ''
  if (hit) return list.map((r) => (r === hit ? { emoji, actors: [...r.actors, me], actorTypes: [...r.actorTypes, 'user'] } : r))
  return [...list, { emoji, actors: [me], actorTypes: ['user'] }]
}

/** Bring the server's identity fields onto rows the client drew itself —
 *  WITHOUT replacing what is on screen. After a reader stop the server's row
 *  is still streaming; a full sync would hand back its longer prose, but the
 *  stopped turn still needs its id (to be reacted to) and its real time.
 *  Rows pair by index and only when the roles agree. */
export function stampFromServer(local: readonly DisplayMessage[], remote: readonly StoredMessage[]): DisplayMessage[] {
  return local.map((m, i) => {
    const r = remote[i]
    if (!r || r.role !== m.role) return m
    return {
      ...m,
      id: r.id ?? m.id,
      createdAt: r.createdAt ?? m.createdAt,
      seq: r.seq ?? m.seq,
      reactions: r.reactions ?? m.reactions,
    }
  })
}

// One formatter per resolved zone ('' = the browser's) — turnTime runs per row.
const turnTimeFormatters = new Map<string, Intl.DateTimeFormat>()

/** A turn's send time beside its name (R2), in the viewer's zone when it is
 *  one Intl can resolve, else the browser's. Empty when there is no time. */
export function turnTime(at: string | undefined, timeZone?: string | null): string {
  if (!at) return ''
  const d = new Date(at)
  if (Number.isNaN(d.getTime())) return ''
  const tz = zoneOrUndefined(timeZone)
  const k = tz ?? ''
  let f = turnTimeFormatters.get(k)
  if (!f) {
    f = new Intl.DateTimeFormat([], { hour: '2-digit', minute: '2-digit', timeZone: tz })
    turnTimeFormatters.set(k, f)
  }
  return f.format(d)
}

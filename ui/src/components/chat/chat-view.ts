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
  /** The reader stopped this turn (the stop tile, or Esc). It lands `complete`
   *  with whatever had arrived, so the row needs its own word: stopping is a
   *  decision, not a fault, and it reads in the muted voice where an
   *  interrupted turn used to shout in danger red. */
  stopped?: boolean
  /** A user row said INTO a reply that was already streaming (POST …/steer)
   *  rather than at the top of a turn — the server's own stamp on the row, so
   *  it survives the post-turn sync. The row wears a notice, because a message
   *  sitting inside somebody else's turn otherwise reads as a transcript out
   *  of order. It changes nothing about the live-resume poller: the steered
   *  row is still owed its own answer, and the server chains one. */
  steer?: boolean
  chips?: ChatChip[]
  /** THE SCHEDULE THAT WROTE THIS TURN — the job id out of the server's cron
   *  marker. A scheduled run posts an ordinary agent turn into the thread
   *  (that is the whole design: the cron's output needs no client of its
   *  own), so without this the row reads as the agent having spoken up by
   *  itself. Absent on every turn somebody actually asked for. */
  cronJobId?: string
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

/** The row stamps the server writes into `metadata`, read through one
 *  accessor so an absent bag is an absent stamp rather than three
 *  `?.`-chains: `resumed` from the auto-resume, `stopped` from the stop
 *  route's drain, `steer` from the user row the steer route pushes. The names
 *  are declared on `StoredMessage.metadata` — the wire type — because that is
 *  what the server sends, and a cast here would have let the two drift. */
const stamps = (m: StoredMessage): NonNullable<StoredMessage['metadata']> => m.metadata ?? {}

/** THE CRON STAMP, out of the row's `marker` — the same `metadata` bag the
 *  three stamps above ride in, and the key `post_agent_turn` writes as its
 *  idempotency key. The server names a scheduled turn
 *  `cron:<job>:<execution>:<turn>` (api cron_output.rs: the marker names the
 *  execution, so a re-delivery of the same run is the same row), and the JOB
 *  is the second field — the only part of it a reader has any use for. A
 *  marker of any other shape, or one whose job field is empty, is not a
 *  scheduled run and gets no stamp. */
export function cronJobFromMarker(raw: string | undefined | null): string | undefined {
  if (!raw?.startsWith('cron:')) return undefined
  const job = raw.split(':')[1]?.trim()
  return job || undefined
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
  resumed: stamps(m).resumed === true,
  stopped: stamps(m).stopped === true,
  steer: stamps(m).steer === true,
  cronJobId: cronJobFromMarker(stamps(m).marker),
  chips: m.chips,
  authorUserId: m.authorUserId,
  id: m.id,
  createdAt: m.createdAt,
  reactions: m.reactions,
})

/** What the composer meant by this submit: Enter (or the send tile), or the
 *  explicit mid-turn send — ⌘/Ctrl+⏎ and the "Send now" control. */
export type SendIntent = 'enter' | 'send-now'

/** Where one submitted message goes. `fresh` starts a turn; `queue` hands it
 *  to the server's queue behind the reply that is streaming; `steer` hands it
 *  to that reply itself; `hold` keeps it on the client until a conversation id
 *  exists. */
export type SendRoute = 'fresh' | 'queue' | 'steer' | 'hold'

/** The whole routing decision for one submit, so the view dispatches instead
 *  of re-deriving it in the middle of a send.
 *
 *  Enter NEVER interrupts — it queues, as it always has; only an explicit
 *  send-now steers. Two things disqualify a steer even then: no conversation
 *  id yet, because the first turn's response headers are the only place one
 *  comes from and a steer has no address until it lands (the message is held
 *  and flushed the moment it does), and pending attachments, because the steer
 *  wire carries `{ content }` and nothing else — a draft with files goes the
 *  queue, which takes them. */
export function sendRoute(ctx: {
  streaming: boolean
  intent: SendIntent
  hasConversation: boolean
  hasAttachments?: boolean
}): SendRoute {
  if (!ctx.streaming) return 'fresh'
  if (!ctx.hasConversation) return 'hold'
  return ctx.intent === 'send-now' && !ctx.hasAttachments ? 'steer' : 'queue'
}

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

/** THE AGENT'S VERDICT ON A CALL, BROUGHT ONTO THE ROW ON SCREEN. Hermes'
 *  completions stream calls every finished tool `completed`, a command its
 *  approval gate refused included. The agent's talaria-events plugin tells
 *  the server otherwise, and the server marks that call `error` on the stored
 *  row (talaria-conversations `record_tool_failure`) and rings the
 *  conversation's readers. A row this tab drew from the stream still says
 *  `completed` until something brings the server's word over: this, on that
 *  ring and on the live turn's poll.
 *
 *  Rows pair by id. A row this tab drew has no id until the post-turn stamp,
 *  and pairs by index with the server's row in its place, when that row is
 *  the agent's and not one a row on screen already holds: stampFromServer's
 *  pairing, so a verdict only ever comes off the row that stamp would take
 *  the id from. Never the server's newest row: an earlier reply not stamped
 *  yet, a reply chained after a steered row, or one this tab has not drawn
 *  is each the newest at some moment, and Hermes mints a turn's call ids
 *  again on a later turn, so the wrong turn's call would be marked. A call
 *  pairs by its tool-call id. Only ever toward `error`, and only a call's
 *  status: the prose, chips and every other field stay what is on screen.
 *  Answers `local` itself when nothing changed, so the caller can skip the
 *  write. */
export function foldToolVerdicts(local: DisplayMessage[], remote: readonly StoredMessage[]): DisplayMessage[] {
  const failedOn = (r: StoredMessage | undefined): Set<string> =>
    new Set((r?.tools ?? []).filter((t) => t.status === 'error' && t.id).map((t) => t.id!))
  const byId = new Map(remote.filter((r) => r.role === 'assistant' && r.id).map((r) => [r.id!, r]))
  const shown = new Set(local.map((m) => m.id).filter(Boolean))
  let changed = false
  const next = local.map((m, i) => {
    if (m.role !== 'assistant' || !m.tools?.length) return m
    const inPlace = remote[i]
    const pair = m.id
      ? byId.get(m.id)
      : inPlace?.role === 'assistant' && !(inPlace.id && shown.has(inPlace.id))
        ? inPlace
        : undefined
    const failed = failedOn(pair)
    if (failed.size === 0 || !m.tools.some((t) => t.id && t.status !== 'error' && failed.has(t.id))) return m
    changed = true
    return { ...m, tools: m.tools.map((t) => (t.id && failed.has(t.id) ? { ...t, status: 'error' as const } : t)) }
  })
  return changed ? next : local
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

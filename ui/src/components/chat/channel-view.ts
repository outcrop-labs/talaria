// Shared helpers for <ChannelView> and its message rows (see
// ChannelView.svelte / MessageRow.svelte / ThreadPanel.svelte).
import { copyAppLink, copyTextItems, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
import { confirm } from '@/components/ui/confirm.svelte'
import { deleteChannelMessage, type ChannelMessage } from '@/lib/channels.svelte'

// Everything a message row needs to know about the room it's in.
export interface MessageCtx {
  channelId: string
  /** The viewer's author identity (email) — gates edit/delete/mine-highlight. */
  me: string
  isChannelOwner: boolean
  labelFor: (model: string) => string
  userLabel: (author: string) => string
  /** The author's photo URL (users: the directory's effective picture, matched
   *  by email), or null for the initials fallback. */
  pictureFor: (author: string, authorType: string) => string | null
  /** The viewer's display name + email local part — their @mentions render in
   *  the stronger self-mention style (Markdown `selfMentions`). */
  selfMentions: string[]
  /** IANA zone for date dividers (profile preference), else the browser's. */
  timeZone?: string | null
  /** Set when the host can show a profile (Comms' drawer): the author's name
   *  and avatar become buttons. Users arrive by email, agents by model. */
  onOpenProfile?: (author: string, authorType: 'user' | 'agent') => void
}

export const actorLabel = (ctx: MessageCtx, actor: string, actorType: string) =>
  actorType === 'agent' ? ctx.labelFor(actor) : ctx.userLabel(actor)

export function rowMenuEntries(m: ChannelMessage, ctx: MessageCtx, openThread: () => void): ContextMenuEntry[] {
  const own = m.authorType === 'user' && m.author === ctx.me
  return [
    ...copyTextItems(m.content),
    { label: 'Copy link', onSelect: () => copyAppLink(`/comms/channel/${ctx.channelId}`) },
    ...(m.threadRootId
      ? []
      : [{ label: 'Reply in thread', onSelect: openThread }]),
    ...(own || ctx.isChannelOwner
      ? [
          'sep' as const,
          {
            label: 'Delete message',
            danger: true,
            onSelect: () => {
              void confirm({
                title: 'Delete message',
                message: m.thread?.count
                  ? `Delete this message and its ${m.thread.count} thread ${m.thread.count === 1 ? 'reply' : 'replies'}?`
                  : 'Delete this message?',
                confirmLabel: 'Delete',
              }).then((ok) => {
                if (ok) void deleteChannelMessage(ctx.channelId, m.id)
              })
            },
          },
        ]
      : []),
  ]
}

/** A thread's participants (emails or agent models) as rollup avatars —
 *  at most `max`, in the order the server lists them. Agent models are any
 *  author not shaped like an email. */
export function threadAvatars(ctx: MessageCtx, authors: readonly string[], max = 3) {
  return authors.slice(0, max).map((a) => {
    const type = a.includes('@') ? 'user' : 'agent'
    return { key: a, name: actorLabel(ctx, a, type), src: ctx.pictureFor(a, type) }
  })
}

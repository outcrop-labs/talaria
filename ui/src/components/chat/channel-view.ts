// Shared helpers for <ChannelView> and its message rows (see
// ChannelView.svelte / MessageRow.svelte / ThreadPanel.svelte).
import { copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
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
}

export const actorLabel = (ctx: MessageCtx, actor: string, actorType: string) =>
  actorType === 'agent' ? ctx.labelFor(actor) : ctx.userLabel(actor)

export function rowMenuEntries(m: ChannelMessage, ctx: MessageCtx, openThread: () => void): ContextMenuEntry[] {
  const own = m.authorType === 'user' && m.author === ctx.me
  return [
    { label: 'Copy text', disabled: !m.content, onSelect: () => void navigator.clipboard.writeText(m.content) },
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

// A quick-react palette, not an emoji browser — Slack-lite on purpose.
export const REACTION_SET = ['👍', '✅', '👀', '🎉', '❤️', '😂', '🚀', '🙏']

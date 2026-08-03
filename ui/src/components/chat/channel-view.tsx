import { useEffect, useRef, useState } from 'react'
import { MessageSquareText, Pencil, SmilePlus, Trash2, X } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { Markdown } from '@/components/ui/markdown'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { confirm } from '@/components/ui/confirm'
import { cn } from '@/lib/cn'
import { useQueryClient } from '@tanstack/react-query'
import {
  deleteChannelMessage,
  editChannelMessage,
  markChannelRead,
  sendChannelMessage,
  toggleMessageReaction,
  useChannelDetail,
  useChannelEvents,
  useChannelMessages,
  useThreadMessages,
  type ChannelMessage,
} from '@/lib/channels'
import { useUsers } from '@/lib/users'
import { useSession } from '@/lib/session'
import { AttachButton, PendingAttachments, MessageAttachments } from '@/components/chat/attachments'
import { GuardCaveat } from '@/components/chat/guard-caveat'
import { KeyHint } from '@/components/ui/kbd'
import { resolveAgentMedia } from '@/lib/agent-media'
import { relativeTime } from '@/lib/fleet'
import { userMentionInsert, type Mentionable } from '@/components/chat/mentions'
import { splitAttachments, uploadFile, type Attachment } from '@/lib/attachments'
import { EmojiButton } from '@/components/chat/emoji'
import { ChatComposer, type ChatComposerHandle } from '@/components/chat/chat-composer'
import type { AgentModel } from '@/lib/agents'

// One channel: live message feed + composer, Slack-shaped. Messages take
// reactions (agents react too), spawn threads (agents @mentioned in a thread
// reply in the thread), and carry pasted/dropped files. Agents reply when
// @mentioned; their streamed replies arrive over the channel's SSE feed.
export function ChannelView({
  channelId,
  channelName,
  fleet,
}: {
  channelId: string
  channelName: string
  fleet: AgentModel[]
}) {
  // The whole query, not `{ data: messages = [] }`. That default turned a
  // rejected fetch into "this channel has no messages" — `isLoading` is false
  // during an error, so the skeleton below (whose own comment says "never the
  // 'Welcome' hero") never ran, and a 500 rendered the hero over four real
  // messages.
  const messagesQuery = useChannelMessages(channelId)
  const messages = messagesQuery.data ?? []
  const messagesLoading = messagesQuery.isLoading
  // Fetched here (not passed down) so the message pane never waits on the
  // parent header's detail fetch — react-query dedupes the shared key.
  const detailQuery = useChannelDetail(channelId)
  const detail = detailQuery.data
  // Same shape, quieter blast radius: a failed detail read empties the agent
  // and member lists, which the hero below reads back as "add people & agents"
  // and the composer as "nobody here to @mention".
  const detailFailed = detailQuery.isError && detail === undefined
  const channelAgents = detail?.agents ?? []
  const members = detail?.members ?? []
  // The one flatten left standing here, deliberately: the directory only maps
  // an author's email to their display name, and the fallback below derives
  // the label from the message's OWN author field. A failed lookup shows
  // "jon" instead of "Jon Iler" — degraded, but not a claim about anything the
  // read failed to fetch. Written as an explicit `?? []` so it can't be
  // mistaken for the destructure-default pattern this round removed.
  const users = useUsers().data ?? []
  const { data: session } = useSession()
  useChannelEvents(channelId)
  const [error, setError] = useState<string | null>(null)
  const [threadRoot, setThreadRoot] = useState<string | null>(null)
  const { openMenu, menu } = useContextMenu()
  const scrollRef = useRef<HTMLDivElement>(null)
  const prevCount = useRef(0)
  const qc = useQueryClient()

  // Having the channel open = having read it: advance the read cursor as
  // messages land, so the sidebar badge clears live.
  const lastReadPosted = useRef<{ id: string; seq: number }>({ id: '', seq: 0 })
  useEffect(() => {
    const latest = messages[messages.length - 1]?.seq ?? 0
    if (!latest) return
    const prev = lastReadPosted.current
    if (prev.id === channelId && latest <= prev.seq) return
    lastReadPosted.current = { id: channelId, seq: latest }
    void markChannelRead(channelId, latest).then(() => qc.invalidateQueries({ queryKey: ['channels'] })).catch(() => {})
  }, [messages, channelId, qc])

  // A thread panel only survives while its root does.
  useEffect(() => {
    if (threadRoot && messages.length > 0 && !messages.some((m) => m.id === threadRoot)) setThreadRoot(null)
  }, [messages, threadRoot])

  // Instant, pinned-only follow (see chat-view): streamed flushes + smooth
  // scrolling rubber-band into a bounce, and reading history must never be
  // yanked back down. A fresh channel load always lands at the end.
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const loaded = prevCount.current === 0 && messages.length > 0
    prevCount.current = messages.length
    const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (loaded || pinned) el.scrollTop = el.scrollHeight
  }, [messages])

  const labelFor = (model: string) => fleet.find((a) => a.id === model)?.label ?? model
  // Human authors are stored by email (stable identity); show their display name.
  const userLabel = (author: string) =>
    users.find((u) => u.email === author)?.name ?? (author.split('@')[0] || author)

  const ctx: MessageCtx = {
    channelId,
    me: session?.email ?? session?.name ?? '',
    isChannelOwner: detail?.role === 'owner',
    labelFor,
    userLabel,
  }

  const mentionables: Mentionable[] = [
    ...channelAgents.map((id) => ({ insert: labelFor(id), label: labelFor(id), sub: id })),
    // Tier mentions: "@Dex:opus" routes the reply to that model tier.
    ...channelAgents.flatMap((id) =>
      (fleet.find((a) => a.id === id)?.tiers ?? []).map((t) => ({
        insert: `${labelFor(id)}:${t}`,
        label: `${labelFor(id)}:${t}`,
        sub: 'tier',
      })),
    ),
    ...members.map((m) => ({
      insert: userMentionInsert(m),
      label: m.name ?? m.email ?? m.userId,
      sub: m.email ?? undefined,
    })),
  ].filter((m) => m.insert)

  const send = async (text: string, atts: Attachment[]) => {
    setError(null)
    try {
      const { attachmentIds, refs } = splitAttachments(atts)
      await sendChannelMessage(channelId, text, attachmentIds, refs)
    } catch (e) {
      setError((e as Error).message)
    }
  }

  return (
    <div className="flex h-full min-h-0">
      {/* One chat width everywhere: the same content token agent DMs use. */}
      <div className="mx-auto flex h-full min-h-0 w-full max-w-[var(--chat-content-max-width)] flex-1 flex-col">
        <div ref={scrollRef} className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
          {/* Transcripts refresh off the channel's SSE ticks. A failed refresh
              keeps the messages already on screen — yanking a conversation
              away over a blip is worse — but says so rather than presenting
              them as the live feed. */}
          {messagesQuery.isError && messagesQuery.data !== undefined && (
            <QueryError
              variant="inline"
              title="Messages may be out of date"
              error={messagesQuery.error}
              onRetry={() => void messagesQuery.refetch()}
            />
          )}
          {detailFailed && (
            <QueryError
              variant="inline"
              title="Could not load this channel's people & agents"
              error={detailQuery.error}
              onRetry={() => void detailQuery.refetch()}
            />
          )}
          {messagesQuery.isError && messagesQuery.data === undefined ? (
            <QueryError
              title="Could not load messages"
              error={messagesQuery.error}
              onRetry={() => void messagesQuery.refetch()}
            />
          ) : messagesLoading ? (
            // Transcript-shaped shimmer while the history loads — never the
            // "Welcome" hero, which reads as an empty channel it isn't.
            <div aria-hidden className="space-y-5">
              {Array.from({ length: 5 }, (_, i) => (
                <div key={i} className="flex gap-2.5">
                  <Skeleton className="mt-0.5 h-7 w-7 shrink-0 rounded-full" delay={i * 0.12} />
                  <div className="min-w-0 flex-1 space-y-2 pt-1">
                    <Skeleton className="h-2.5 w-24 rounded-full" delay={i * 0.12} />
                    <div style={{ width: ['82%', '64%', '90%', '71%', '58%'][i] }}>
                      <Skeleton className="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.06} />
                    </div>
                    <div style={{ width: ['55%', '78%', '40%', '62%', '84%'][i] }}>
                      <Skeleton className="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.12} />
                    </div>
                  </div>
                </div>
              ))}
            </div>
          ) : messages.length === 0 ? (
            <EmptyState
              icon="#"
              title={`Welcome to #${channelName}`}
              hint={
                channelAgents.length
                  ? `Say something. @mention ${channelAgents.map(labelFor).join(', ')} to bring the agents in.`
                  : detailFailed
                    ? // The channel really is empty, but who is IN it came from
                      // a read that failed — "add people & agents" would be a
                      // claim about a roster nobody managed to fetch.
                      'Say something.'
                    : 'Say something, or add people & agents.'
              }
            />
          ) : (
            messages.map((m) => (
              <MessageRow
                key={m.id}
                message={m}
                ctx={ctx}
                onOpenThread={() => setThreadRoot(m.threadRootId ?? m.id)}
                onContextMenu={(e) => openMenu(e, rowMenuEntries(m, ctx, () => setThreadRoot(m.id)))}
              />
            ))
          )}
          {error && (
            <div className="text-center text-sm" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
        </div>

        <Composer channelName={channelName} mentionables={mentionables} onSend={send} />
        {menu}
      </div>

      {threadRoot && (
        <ThreadPanel
          channelId={channelId}
          rootId={threadRoot}
          ctx={ctx}
          mentionables={mentionables}
          onClose={() => setThreadRoot(null)}
        />
      )}
    </div>
  )
}

// Everything a message row needs to know about the room it's in.
interface MessageCtx {
  channelId: string
  /** The viewer's author identity (email) — gates edit/delete/mine-highlight. */
  me: string
  isChannelOwner: boolean
  labelFor: (model: string) => string
  userLabel: (author: string) => string
}

const actorLabel = (ctx: MessageCtx, actor: string, actorType: string) =>
  actorType === 'agent' ? ctx.labelFor(actor) : ctx.userLabel(actor)

function rowMenuEntries(m: ChannelMessage, ctx: MessageCtx, openThread: () => void): ContextMenuEntry[] {
  const own = m.authorType === 'user' && m.author === ctx.me
  return [
    { label: 'Copy text', disabled: !m.content, onSelect: () => void navigator.clipboard.writeText(m.content) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/comms?c=${ctx.channelId}`) },
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
const REACTION_SET = ['👍', '✅', '👀', '🎉', '❤️', '😂', '🚀', '🙏']

function MessageRow({
  message: m,
  ctx,
  inThread = false,
  onOpenThread,
  onContextMenu,
}: {
  message: ChannelMessage
  ctx: MessageCtx
  /** Rendered inside the thread panel: no thread affordances of its own. */
  inThread?: boolean
  onOpenThread?: () => void
  onContextMenu?: (e: React.MouseEvent) => void
}) {
  const name = m.authorType === 'agent' ? ctx.labelFor(m.author) : ctx.userLabel(m.author)
  const time = new Date(m.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  const live = m.status === 'streaming'
  const own = m.authorType === 'user' && m.author === ctx.me
  const [picking, setPicking] = useState(false)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')
  const toolbarRef = useRef<HTMLDivElement>(null)

  // The palette must never trap you: outside click, Esc, or simply moving
  // off the message all dismiss it.
  useEffect(() => {
    if (!picking) return
    const onDoc = (e: MouseEvent) => {
      if (!toolbarRef.current?.contains(e.target as Node)) setPicking(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setPicking(false)
    }
    document.addEventListener('mousedown', onDoc)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDoc)
      document.removeEventListener('keydown', onKey)
    }
  }, [picking])

  const react = (emoji: string) => {
    setPicking(false)
    void toggleMessageReaction(ctx.channelId, m.id, emoji).catch(() => {})
  }
  const saveEdit = () => {
    const text = draft.trim()
    setEditing(false)
    if (text && text !== m.content) void editChannelMessage(ctx.channelId, m.id, text).catch(() => {})
  }

  return (
    <div className="group relative flex gap-2.5" onContextMenu={onContextMenu} onMouseLeave={() => setPicking(false)}>
      <Avatar name={name} className="mt-0.5 shrink-0" />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-sm font-semibold text-fg">{name}</span>
          {m.authorType === 'agent' && (
            <span className="rounded border border-line-subtle px-1 text-[10px] uppercase tracking-wide text-muted">
              agent
            </span>
          )}
          <span className="text-xs text-muted">{time}</span>
          {m.editedAt && <span className="text-[10px] text-muted/80">(edited)</span>}
        </div>
        <div className="text-sm">
          {editing ? (
            <div className="mt-1">
              <Textarea
                autoFocus
                autoGrow
                rows={1}
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault()
                    saveEdit()
                  } else if (e.key === 'Escape') {
                    setEditing(false)
                  }
                }}
                className="max-h-40"
              />
              <div className="mt-1 text-[11px] text-muted">enter to save · esc to cancel</div>
            </div>
          ) : m.content ? (
            <Markdown>{m.authorType === 'agent' ? resolveAgentMedia(m.content, m.author) : m.content}</Markdown>
          ) : live ? (
            <span className="inline-flex gap-1 py-1">
              <Dot /> <Dot delay={0.15} /> <Dot delay={0.3} />
            </span>
          ) : null}
          {m.attachments && m.attachments.length > 0 && <MessageAttachments items={m.attachments} />}
          {!live && <GuardCaveat findings={m.guard} />}
          {m.content && live && <span className="ml-0.5 inline-block h-4 w-1.5 animate-pulse bg-accent align-middle" />}
          {m.status === 'error' && (
            <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
              · interrupted
            </div>
          )}
        </div>

        {/* Reaction chips: click toggles yours; hover names the reactors. */}
        {(m.reactions?.length ?? 0) > 0 && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            {m.reactions!.map((r) => {
              const mine = r.actors.some((a, i) => r.actorTypes[i] === 'user' && a === ctx.me)
              return (
                <button
                  key={r.emoji}
                  type="button"
                  title={r.actors.map((a, i) => actorLabel(ctx, a, r.actorTypes[i] ?? 'user')).join(', ')}
                  onClick={() => react(r.emoji)}
                  className={cn(
                    'flex items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs transition-colors',
                    mine ? 'border-accent bg-accent/10 text-fg' : 'border-line-subtle bg-card/50 text-muted hover:border-line hover:text-fg',
                  )}
                >
                  <span>{r.emoji}</span>
                  <span className="text-[11px]">{r.actors.length}</span>
                </button>
              )
            })}
          </div>
        )}

        {/* Thread rollup on roots (main flow only). */}
        {!inThread && m.thread && (
          <button
            type="button"
            onClick={onOpenThread}
            className="mt-1.5 flex items-center gap-1.5 rounded-lg border border-line-subtle bg-card/50 px-2 py-1 text-xs text-accent transition-colors hover:border-accent"
          >
            <MessageSquareText size={12} />
            {m.thread.count} {m.thread.count === 1 ? 'reply' : 'replies'}
            <span className="text-muted">· {relativeTime(m.thread.lastAt)}</span>
          </button>
        )}
      </div>

      {/* Hover toolbar: react · thread · edit (own) — delete lives in the
          context menu behind a confirm. */}
      {!live && !editing && (
        <div
          ref={toolbarRef}
          className={cn(
            'absolute -top-2 right-0 items-center gap-0.5 rounded-lg border border-line bg-card p-0.5 shadow-sm',
            picking ? 'flex' : 'hidden group-hover:flex',
          )}
        >
          {picking ? (
            REACTION_SET.map((e) => (
              <button
                key={e}
                type="button"
                onClick={() => react(e)}
                className="grid h-7 w-7 place-items-center rounded-md text-base transition-colors hover:bg-sidebar"
              >
                {e}
              </button>
            ))
          ) : (
            <>
              <HoverAction title="Add reaction" onClick={() => setPicking(true)}>
                <SmilePlus size={14} />
              </HoverAction>
              {!inThread && !m.threadRootId && (
                <HoverAction title="Reply in thread" onClick={() => onOpenThread?.()}>
                  <MessageSquareText size={14} />
                </HoverAction>
              )}
              {own && (
                <HoverAction
                  title="Edit message"
                  onClick={() => {
                    setDraft(m.content)
                    setEditing(true)
                  }}
                >
                  <Pencil size={14} />
                </HoverAction>
              )}
              {(own || ctx.isChannelOwner) && (
                <HoverAction
                  title="Delete message"
                  danger
                  onClick={() => {
                    void confirm({
                      title: 'Delete message',
                      message: m.thread?.count
                        ? `Delete this message and its ${m.thread.count} thread ${m.thread.count === 1 ? 'reply' : 'replies'}?`
                        : 'Delete this message?',
                      confirmLabel: 'Delete',
                    }).then((ok) => {
                if (ok) void deleteChannelMessage(ctx.channelId, m.id)
              })
                  }}
                >
                  <Trash2 size={14} />
                </HoverAction>
              )}
            </>
          )}
        </div>
      )}
    </div>
  )
}

function HoverAction({
  title,
  danger,
  onClick,
  children,
}: {
  title: string
  danger?: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={cn(
        'grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-sidebar',
        danger ? 'hover:text-[color:var(--theme-danger)]' : 'hover:text-fg',
      )}
    >
      {children}
    </button>
  )
}

/** The thread side panel: root + replies + its own composer. Replies are
 *  channel messages that hang off the root; @mentioned agents answer HERE. */
function ThreadPanel({
  channelId,
  rootId,
  ctx,
  mentionables,
  onClose,
}: {
  channelId: string
  rootId: string
  ctx: MessageCtx
  mentionables: Mentionable[]
  onClose: () => void
}) {
  // Same trap one level down: `= []` on a rejected thread read rendered an
  // EMPTY thread panel — no root, no replies, no error — beside a rollup that
  // had just said "3 replies".
  const threadQuery = useThreadMessages(channelId, rootId)
  const messages = threadQuery.data ?? []
  const isLoading = threadQuery.isLoading
  const scrollRef = useRef<HTMLDivElement>(null)
  const prevCount = useRef(0)

  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const loaded = prevCount.current === 0 && messages.length > 0
    prevCount.current = messages.length
    const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (loaded || pinned) el.scrollTop = el.scrollHeight
  }, [messages])

  const send = async (text: string, atts: Attachment[]) => {
    const { attachmentIds, refs } = splitAttachments(atts)
    await sendChannelMessage(channelId, text, attachmentIds, refs, rootId)
  }

  return (
    <div className="flex w-[380px] shrink-0 flex-col border-l border-line-subtle bg-surface/50">
      <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2.5">
        <MessageSquareText size={14} className="text-muted" />
        <span className="text-sm font-semibold text-fg">Thread</span>
        <button
          type="button"
          onClick={onClose}
          className="ml-auto grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:bg-card hover:text-fg"
          title="Close thread"
        >
          <X size={14} />
        </button>
      </div>
      <div ref={scrollRef} className="flex-1 space-y-4 overflow-y-auto px-4 py-4">
        {threadQuery.isError && threadQuery.data !== undefined && (
          <QueryError
            variant="inline"
            title="Thread may be out of date"
            error={threadQuery.error}
            onRetry={() => void threadQuery.refetch()}
          />
        )}
        {threadQuery.isError && threadQuery.data === undefined ? (
          <QueryError
            variant="compact"
            title="Could not load this thread"
            error={threadQuery.error}
            onRetry={() => void threadQuery.refetch()}
          />
        ) : isLoading ? (
          <div aria-hidden className="space-y-4">
            {Array.from({ length: 3 }, (_, i) => (
              <div key={i} className="flex gap-2.5">
                <Skeleton className="mt-0.5 h-7 w-7 shrink-0 rounded-full" delay={i * 0.12} />
                <div className="min-w-0 flex-1 space-y-2 pt-1">
                  <Skeleton className="h-2.5 w-20 rounded-full" delay={i * 0.12} />
                  <Skeleton className="h-2.5 w-4/5 rounded-full" delay={i * 0.12 + 0.06} />
                </div>
              </div>
            ))}
          </div>
        ) : (
          messages.map((m, i) => (
            <div key={m.id}>
              <MessageRow message={m} ctx={ctx} inThread />
              {i === 0 && messages.length > 1 && (
                <div className="mt-3 border-t border-line-subtle pt-1 text-[11px] text-muted">
                  {messages.length - 1} {messages.length - 1 === 1 ? 'reply' : 'replies'}
                </div>
              )}
            </div>
          ))
        )}
      </div>
      <Composer channelName="thread" placeholder="Reply in thread. @mention an agent to bring it in" mentionables={mentionables} onSend={send} />
    </div>
  )
}

/** The channel composer: the Slack-shaped rich editor (chat-composer.tsx)
 *  plus attachment chips. Files paste and drop straight in — images from the
 *  clipboard, files from the desktop — uploading immediately as pending chips. */
function Composer({
  channelName,
  placeholder,
  mentionables,
  onSend,
}: {
  channelName: string
  placeholder?: string
  mentionables: Mentionable[]
  onSend: (text: string, attachments: Attachment[]) => Promise<void>
}) {
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [empty, setEmpty] = useState(true)
  const [dragging, setDragging] = useState(false)
  const editorRef = useRef<ChatComposerHandle>(null)

  const uploadAll = (files: Iterable<File>) => {
    for (const f of files) {
      void uploadFile(f).then((r) => {
        if ('id' in r) setAttachments((prev) => [...prev, r])
      })
    }
  }

  const submit = (markdown: string) => {
    if (!markdown && attachments.length === 0) return
    const atts = attachments
    setAttachments([])
    editorRef.current?.clear()
    void onSend(markdown, atts)
  }

  return (
    <div className="relative px-6 pb-6">
      <div
        className={cn('mercury-panel rounded-2xl p-2 transition-colors', dragging && 'border-accent bg-accent/5')}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes('Files')) {
            e.preventDefault()
            setDragging(true)
          }
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={() => setDragging(false)}
      >
        <PendingAttachments items={attachments} onRemove={(id) => setAttachments((prev) => prev.filter((a) => a.id !== id))} />
        <ChatComposer
          ref={editorRef}
          placeholder={placeholder ?? `Message #${channelName}. @mention an agent to bring it in`}
          mentionables={mentionables}
          onSubmit={submit}
          onFiles={uploadAll}
          onEmptyChange={setEmpty}
          leftControls={
            <>
              <AttachButton onAttach={(a) => setAttachments((prev) => [...prev, a])} />
              <EmojiButton onPick={(ch) => editorRef.current?.insertText(ch)} />
            </>
          }
          rightControls={<KeyHint keys="⏎" label="send" visible={!empty || attachments.length > 0} />}
        />
      </div>
    </div>
  )
}

function Dot({ delay = 0 }: { delay?: number }) {
  return <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-muted" style={{ animationDelay: `${delay}s` }} />
}

import { useEffect, useRef, useState } from 'react'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Markdown } from '@/components/ui/markdown'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { sendChannelMessage, useChannelEvents, useChannelMessages, type ChannelMember, type ChannelMessage } from '@/lib/channels'
import { useUsers } from '@/lib/users'
import { AttachButton, PendingAttachments, MessageAttachments } from '@/components/chat/attachments'
import { resolveAgentMedia } from '@/lib/agent-media'
import { MentionMenu, useMentions, userMentionInsert, type Mentionable } from '@/components/chat/mentions'
import type { Attachment } from '@/lib/attachments'
import type { AgentModel } from '@/lib/agents'

// One channel: live message feed + composer. Agents reply when @mentioned;
// their streamed replies arrive over the channel's SSE feed like anyone else's.
// @mentioning a human member drops a notification in their inbox.
export function ChannelView({
  channelId,
  channelName,
  channelAgents,
  members,
  fleet,
}: {
  channelId: string
  channelName: string
  channelAgents: string[]
  members: ChannelMember[]
  fleet: AgentModel[]
}) {
  const { data: messages = [] } = useChannelMessages(channelId)
  const { data: users = [] } = useUsers()
  useChannelEvents(channelId)
  const [error, setError] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const prevCount = useRef(0)

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

  const send = async (text: string, attachmentIds: string[]) => {
    setError(null)
    try {
      await sendChannelMessage(channelId, text, attachmentIds)
    } catch (e) {
      setError((e as Error).message)
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div ref={scrollRef} className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
        {messages.length === 0 ? (
          <EmptyState
            icon="#"
            title={`Welcome to #${channelName}`}
            hint={
              channelAgents.length
                ? `Say something — @mention ${channelAgents.map(labelFor).join(', ')} to bring the agents in.`
                : 'Say something, or add people & agents.'
            }
          />
        ) : (
          messages.map((m) => <MessageRow key={m.id} message={m} labelFor={labelFor} userLabel={userLabel} />)
        )}
        {error && (
          <div className="text-center text-sm" style={{ color: 'var(--theme-danger)' }}>
            {error}
          </div>
        )}
      </div>

      <Composer
        channelName={channelName}
        mentionables={[
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
        ].filter((m) => m.insert)}
        onSend={send}
      />
    </div>
  )
}

function MessageRow({
  message: m,
  labelFor,
  userLabel,
}: {
  message: ChannelMessage
  labelFor: (model: string) => string
  userLabel: (author: string) => string
}) {
  const name = m.authorType === 'agent' ? labelFor(m.author) : userLabel(m.author)
  const time = new Date(m.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  const live = m.status === 'streaming'
  return (
    <div className="flex gap-2.5">
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
        </div>
        <div className="text-sm">
          {m.content ? (
            <Markdown>{m.authorType === 'agent' ? resolveAgentMedia(m.content, m.author) : m.content}</Markdown>
          ) : live ? (
            <span className="inline-flex gap-1 py-1">
              <Dot /> <Dot delay={0.15} /> <Dot delay={0.3} />
            </span>
          ) : null}
          {m.attachments && m.attachments.length > 0 && <MessageAttachments items={m.attachments} />}
          {m.content && live && <span className="ml-0.5 inline-block h-4 w-1.5 animate-pulse bg-accent align-middle" />}
          {m.status === 'error' && (
            <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
              · interrupted
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

/** Composer with @mention autocomplete over the channel's agents and members. */
function Composer({
  channelName,
  mentionables,
  onSend,
}: {
  channelName: string
  mentionables: Mentionable[]
  onSend: (text: string, attachmentIds: string[]) => Promise<void>
}) {
  const [input, setInput] = useState('')
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [caret, setCaret] = useState(0)
  const taRef = useRef<HTMLTextAreaElement>(null)

  const { mention, picked, insert, onKeyDown: onMentionKey } = useMentions(input, caret, setCaret, mentionables, (next, pos) => {
    setInput(next)
    requestAnimationFrame(() => {
      taRef.current?.focus()
      taRef.current?.setSelectionRange(pos, pos)
      setCaret(pos)
    })
  })

  const send = () => {
    const text = input.trim()
    if (!text && attachments.length === 0) return
    const atts = attachments
    setInput('')
    setAttachments([])
    void onSend(text, atts.map((a) => a.id))
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (onMentionKey(e)) return
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      send()
    }
  }

  const trackCaret = () => setCaret(taRef.current?.selectionStart ?? 0)

  return (
    <div className="relative px-6 pb-6">
      {mention && <MentionMenu mention={mention} picked={picked} onPick={insert} className="absolute bottom-full left-4 mb-1" />}
      <div className="mercury-panel rounded-2xl p-2">
        <PendingAttachments items={attachments} onRemove={(id) => setAttachments((prev) => prev.filter((a) => a.id !== id))} />
        <div className="flex items-end gap-2">
          <AttachButton onAttach={(a) => setAttachments((prev) => [...prev, a])} />
          <Textarea
            ref={taRef}
            autoGrow
            rows={1}
            value={input}
            onChange={(e) => {
              setInput(e.target.value)
              trackCaret()
            }}
            onKeyUp={trackCaret}
            onClick={trackCaret}
            onKeyDown={onKeyDown}
            placeholder={`Message #${channelName} — @mention an agent to bring it in`}
            className="max-h-40 min-h-[2.75rem] border-0 bg-transparent focus:border-0"
          />
          <Button onClick={send} disabled={!input.trim() && attachments.length === 0}>
            Send
          </Button>
        </div>
      </div>
    </div>
  )
}

function Dot({ delay = 0 }: { delay?: number }) {
  return <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-muted" style={{ animationDelay: `${delay}s` }} />
}

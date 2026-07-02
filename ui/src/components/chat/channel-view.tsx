import { useEffect, useMemo, useRef, useState } from 'react'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Markdown } from '@/components/ui/markdown'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { sendChannelMessage, useChannelEvents, useChannelMessages, type ChannelMember, type ChannelMessage } from '@/lib/channels'
import { useUsers } from '@/lib/users'
import type { AgentModel } from '@/lib/agents'

/** A composer mention option: `insert` is the token typed into the message. */
interface Mentionable {
  insert: string
  label: string
  sub?: string
}

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
  const bottomRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  const labelFor = (model: string) => fleet.find((a) => a.id === model)?.label ?? model
  // Human authors are stored by email (stable identity); show their display name.
  const userLabel = (author: string) =>
    users.find((u) => u.email === author)?.name ?? (author.split('@')[0] || author)

  const send = async (text: string) => {
    setError(null)
    try {
      await sendChannelMessage(channelId, text)
    } catch (e) {
      setError((e as Error).message)
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
        {messages.length === 0 ? (
          <EmptyState
            icon="#"
            title={`Welcome to #${channelName}`}
            hint={
              channelAgents.length
                ? `Say something — @mention ${channelAgents.map(labelFor).join(', ')} to bring the agents in.`
                : 'Say something, or add agents from channel settings.'
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
        <div ref={bottomRef} />
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
            // Mirror the server's mention tokens: email localpart, else dashed name.
            insert: m.email?.split('@')[0] ?? (m.name ?? '').toLowerCase().replace(/\s+/g, '-'),
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
            <Markdown>{m.content}</Markdown>
          ) : live ? (
            <span className="inline-flex gap-1 py-1">
              <Dot /> <Dot delay={0.15} /> <Dot delay={0.3} />
            </span>
          ) : null}
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
  onSend: (text: string) => Promise<void>
}) {
  const [input, setInput] = useState('')
  const [caret, setCaret] = useState(0)
  const [picked, setPicked] = useState(0)
  const taRef = useRef<HTMLTextAreaElement>(null)

  // An "@word" immediately before the caret opens the mention menu.
  const mention = useMemo(() => {
    const upto = input.slice(0, caret)
    const m = /(^|\s)@([a-z0-9-]*(?::[a-z0-9-]*)?)$/i.exec(upto)
    if (!m) return null
    const q = m[2]!.toLowerCase()
    const options = mentionables.filter(
      (a) => a.label.toLowerCase().startsWith(q) || a.insert.toLowerCase().startsWith(q),
    )
    return options.length ? { start: upto.length - m[2]!.length - 1, options } : null
  }, [input, caret, mentionables])

  useEffect(() => setPicked(0), [mention?.options.length])

  const insertMention = (label: string) => {
    if (!mention) return
    const next = `${input.slice(0, mention.start)}@${label} ${input.slice(caret)}`
    setInput(next)
    const pos = mention.start + label.length + 2
    requestAnimationFrame(() => {
      taRef.current?.focus()
      taRef.current?.setSelectionRange(pos, pos)
      setCaret(pos)
    })
  }

  const send = () => {
    const text = input.trim()
    if (!text) return
    setInput('')
    void onSend(text)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (mention) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        const d = e.key === 'ArrowDown' ? 1 : -1
        setPicked((p) => (p + d + mention.options.length) % mention.options.length)
        return
      }
      if (e.key === 'Tab' || e.key === 'Enter') {
        e.preventDefault()
        insertMention(mention.options[picked]!.insert)
        return
      }
      if (e.key === 'Escape') return setCaret(0)
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      send()
    }
  }

  const trackCaret = () => setCaret(taRef.current?.selectionStart ?? 0)

  return (
    <div className="relative px-6 pb-6">
      {mention && (
        <div className="mercury-panel absolute bottom-full left-4 z-10 mb-1 w-64 overflow-hidden rounded-xl p-1">
          {mention.options.map((a, i) => (
            <button
              key={`${a.insert}-${a.sub ?? ''}`}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault()
                insertMention(a.insert)
              }}
              className={cn(
                'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm',
                i === picked ? 'bg-card text-fg' : 'text-muted',
              )}
            >
              <Avatar name={a.label} className="h-5 w-5 text-xs" />
              <span className="truncate">{a.label}</span>
              {a.sub && <span className="ml-auto truncate text-xs text-muted">{a.sub}</span>}
            </button>
          ))}
        </div>
      )}
      <div className="mercury-panel flex items-end gap-2 rounded-2xl p-2">
        <Textarea
          ref={taRef}
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
        <Button onClick={send} disabled={!input.trim()}>
          Send
        </Button>
      </div>
    </div>
  )
}

function Dot({ delay = 0 }: { delay?: number }) {
  return <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-muted" style={{ animationDelay: `${delay}s` }} />
}

import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { TierPicker } from '@/components/chat/tier-picker'
import { MentionMenu, useMentions, type Mentionable } from '@/components/chat/mentions'
import { AttachButton, PendingAttachments, MessageAttachments } from '@/components/chat/attachments'
import { Markdown } from '@/components/ui/markdown'
import { Disclosure } from '@/components/ui/disclosure'
import { resolveAgentMedia } from '@/lib/agent-media'
import { queueChatMessage, streamChat } from '@/lib/chat'
import { mergeTool, type ToolCall } from '@/lib/sse-parse'
import { loadConversation, type StoredMessage } from '@/lib/conversations'
import type { Attachment } from '@/lib/attachments'

interface DisplayMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning?: string
  tools?: ToolCall[]
  status?: 'streaming' | 'complete' | 'error'
  attachments?: Attachment[]
}

const toDisplay = (m: StoredMessage): DisplayMessage => ({
  role: m.role,
  content: m.content,
  reasoning: m.reasoning,
  tools: m.tools,
  status: m.status,
  attachments: m.attachments,
})

// A durable chat thread. Server owns history; this loads an existing conversation
// (conversationId) or starts fresh (newChatSignal), and streams new turns.
export function ChatView({
  agentModel,
  agentLabel,
  tiers = [],
  conversationId,
  newChatSignal,
  onCreated,
  kind = 'chat',
  headerAction,
  mentionables = [],
}: {
  agentModel: string
  agentLabel: string
  /** Requestable model tiers for this agent (alias names). */
  tiers?: string[]
  conversationId: string | null
  newChatSignal: number
  onCreated: (id: string) => void
  /** 'plan' conversations live in the Plan surface and draft tickets. */
  kind?: 'chat' | 'plan'
  /** Optional actions rendered in a top bar (e.g. Plan's "Draft tickets"). */
  headerAction?: React.ReactNode
  /** Composer @mention options (e.g. the plan surface offers teammates). */
  mentionables?: Mentionable[]
}) {
  const [messages, setMessages] = useState<DisplayMessage[]>([])
  const [input, setInput] = useState('')
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [tier, setTier] = useState('') // '' = the agent's main model
  const [streaming, setStreaming] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [caret, setCaret] = useState(0)
  const abortRef = useRef<AbortController | null>(null)
  const convIdRef = useRef<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const prevCount = useRef(0)
  const taRef = useRef<HTMLTextAreaElement>(null)

  // Follow the stream WITHOUT smooth-scrolling: token flushes fire this every
  // few ms, and overlapping smooth animations rubber-band (the "bounce").
  // Instant jumps, and only while pinned near the bottom — scrolling up to
  // read history is never yanked away. A fresh load always lands at the end.
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const loaded = prevCount.current === 0 && messages.length > 0
    prevCount.current = messages.length
    const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (loaded || pinned) el.scrollTop = el.scrollHeight
  }, [messages])
  useEffect(() => () => abortRef.current?.abort(), [])

  // New-chat reset (declared before the loader so mount order is reset→load).
  useEffect(() => {
    abortRef.current?.abort()
    convIdRef.current = null
    setMessages([])
    setError(null)
  }, [newChatSignal])

  // Load an existing conversation when the selection changes.
  useEffect(() => {
    if (!conversationId || conversationId === convIdRef.current) return
    abortRef.current?.abort()
    convIdRef.current = conversationId
    let cancelled = false
    loadConversation(conversationId).then((res) => {
      if (!cancelled) setMessages((res?.messages ?? []).map(toDisplay))
    })
    return () => {
      cancelled = true
    }
  }, [conversationId])

  // Live-resume: if a loaded reply is still 'streaming' server-side (we didn't
  // start it — a reload landed mid-generation), poll the persisted state so it
  // fills in live until the server finalizes it. Capped so it can't poll forever.
  const last = messages[messages.length - 1]
  const resuming = !streaming && last?.role === 'assistant' && last.status === 'streaming'
  useEffect(() => {
    if (!resuming) return
    const id = convIdRef.current
    if (!id) return
    let stop = false
    let ticks = 0
    const iv = setInterval(async () => {
      if (stop || ++ticks > 300) return clearInterval(iv) // ~4 min — long agent replies keep animating
      const res = await loadConversation(id)
      if (!stop && res) setMessages(res.messages.map(toDisplay))
    }, 800)
    return () => {
      stop = true
      clearInterval(iv)
    }
  }, [resuming])

  // Refresh from the server's truth: queued messages, and any follow-up turn
  // the server chained (the resuming poller then animates it live).
  const syncFromServer = async () => {
    const id = convIdRef.current
    if (!id) return
    const res = await loadConversation(id)
    if (res && convIdRef.current === id) setMessages(res.messages.map(toDisplay))
  }

  const send = async () => {
    const text = input.trim()
    if (!text && attachments.length === 0) return
    const atts = attachments
    setError(null)
    setInput('')
    setAttachments([])

    // Claude-style flow: sending while the agent is replying never interrupts —
    // the message queues into history and the agent picks it up next turn.
    if (streaming && convIdRef.current) {
      setMessages((prev) => [...prev, { role: 'user', content: text, attachments: atts }])
      try {
        await queueChatMessage({
          model: agentModel,
          conversationId: convIdRef.current,
          content: text,
          tier: tier || undefined,
          attachmentIds: atts.map((a) => a.id),
          kind,
        })
      } catch (e) {
        setError((e as Error).message)
      }
      return
    }

    setMessages((prev) => [
      ...prev,
      { role: 'user', content: text, attachments: atts },
      { role: 'assistant', content: '', reasoning: '', tools: [], status: 'streaming' },
    ])
    setStreaming(true)

    const patchLast = (fn: (m: DisplayMessage) => DisplayMessage) =>
      setMessages((prev) => {
        const copy = prev.slice()
        const last = copy[copy.length - 1]
        if (last?.role === 'assistant') copy[copy.length - 1] = fn(last)
        return copy
      })

    const ctrl = new AbortController()
    abortRef.current = ctrl
    try {
      for await (const ev of streamChat(
        { model: agentModel, conversationId: convIdRef.current ?? undefined, content: text, tier: tier || undefined, attachmentIds: atts.map((a) => a.id), kind },
        (meta) => {
          if (!convIdRef.current) {
            convIdRef.current = meta.conversationId
            onCreated(meta.conversationId)
          }
        },
        ctrl.signal,
      )) {
        if (ev.type === 'content') patchLast((m) => ({ ...m, content: m.content + ev.text }))
        else if (ev.type === 'reasoning') patchLast((m) => ({ ...m, reasoning: (m.reasoning ?? '') + ev.text }))
        else if (ev.type === 'tool') patchLast((m) => ({ ...m, tools: mergeTool(m.tools ?? [], ev) }))
        else if (ev.type === 'queued') {
          // A reply was already streaming server-side (e.g. a chained turn we
          // hadn't seen) — drop the placeholder; the sync below shows reality.
          setMessages((prev) => (prev[prev.length - 1]?.role === 'assistant' ? prev.slice(0, -1) : prev))
          break
        }
      }
      // A stream that ends having produced NOTHING is a failure, not a reply —
      // typically the agent's model isn't routable. Say so instead of leaving
      // a silent empty bubble (reads as a frozen chat).
      patchLast((m) => ({
        ...m,
        status: m.content || m.reasoning?.trim() || m.tools?.length ? 'complete' : 'error',
      }))
    } catch (e) {
      if ((e as Error).name !== 'AbortError') setError((e as Error).message)
    } finally {
      setStreaming(false)
      abortRef.current = null
      // Pick up whatever happened meanwhile: queued messages, and the
      // follow-up turn the server chains for them (the resuming poller
      // animates it live once it appears).
      void syncFromServer()
    }
  }

  const stop = () => abortRef.current?.abort()

  const { mention, picked, insert, onKeyDown: onMentionKey } = useMentions(input, caret, setCaret, mentionables, (next, pos) => {
    setInput(next)
    requestAnimationFrame(() => {
      taRef.current?.focus()
      taRef.current?.setSelectionRange(pos, pos)
      setCaret(pos)
    })
  })
  const trackCaret = () => setCaret(taRef.current?.selectionStart ?? 0)

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (onMentionKey(e)) return
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void send()
    }
  }

  return (
    <div className="mx-auto flex h-full w-full max-w-[var(--chat-content-max-width)] flex-col">
      {headerAction && (
        <div className="flex items-center justify-end gap-2 border-b border-line-subtle px-6 py-2">{headerAction}</div>
      )}
      <div ref={scrollRef} className="flex-1 space-y-5 overflow-y-auto px-6 py-6">
        {messages.length === 0 ? (
          <div className="grid h-full place-items-center text-center">
            <div>
              <div className="mercury-text mb-1 text-lg font-semibold">
                {kind === 'plan' ? `Plan with ${agentLabel}` : `Talk to ${agentLabel}`}
              </div>
              <div className="text-sm text-muted">
                {kind === 'plan'
                  ? 'Think through the work together, then draft tickets and send them to a board.'
                  : 'Ask anything — memory, skills, and tools intact.'}
              </div>
            </div>
          </div>
        ) : (
          messages.map((m, i) =>
            m.role === 'user' ? (
              <UserBubble key={i} content={m.content} attachments={m.attachments} />
            ) : (
              <AssistantTurn key={i} message={m} agentModel={agentModel} live={(streaming || resuming) && i === messages.length - 1} />
            ),
          )
        )}
        {error && <div className="text-center text-sm" style={{ color: 'var(--theme-danger)' }}>{error}</div>}
      </div>

      <div className="relative px-6 pb-6">
        {mention && <MentionMenu mention={mention} picked={picked} onPick={insert} className="absolute bottom-full left-4 mb-1" />}
        <div className="mercury-panel rounded-2xl p-2">
          <PendingAttachments items={attachments} onRemove={(id) => setAttachments((prev) => prev.filter((a) => a.id !== id))} />
          <div className="flex items-end gap-2">
            <AttachButton onAttach={(a) => setAttachments((prev) => [...prev, a])} disabled={streaming} />
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
              placeholder={`Message ${agentLabel}…`}
              className="max-h-40 min-h-[2.75rem] border-0 bg-transparent focus:border-0"
            />
            {tiers.length > 0 && <TierPicker tiers={tiers} value={tier} onChange={setTier} />}
            {streaming && (
              <Button variant="outline" onClick={stop}>Stop</Button>
            )}
            <Button onClick={() => void send()} disabled={!input.trim() && attachments.length === 0}>
              Send
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}

function UserBubble({ content, attachments }: { content: string; attachments?: Attachment[] }) {
  return (
    <div className="flex justify-end">
      <div
        className="max-w-[85%] whitespace-pre-wrap rounded-2xl border px-4 py-2.5 text-sm text-[color:var(--chat-user-foreground)]"
        style={{ background: 'var(--chat-user-bg)', borderColor: 'var(--chat-user-border)' }}
      >
        {content}
        {attachments && attachments.length > 0 && <MessageAttachments items={attachments} />}
      </div>
    </div>
  )
}

function AssistantTurn({ message, agentModel, live }: { message: DisplayMessage; agentModel: string; live: boolean }) {
  const { content, reasoning, tools, status } = message
  const hasReasoning = !!reasoning?.trim()
  const hasTools = !!tools?.length
  const empty = !content && !hasReasoning && !hasTools

  return (
    <div className="flex justify-start">
      <div
        className="max-w-[85%] space-y-2 rounded-2xl border px-4 py-2.5 text-sm text-[color:var(--chat-assistant-foreground)]"
        style={{ background: 'var(--chat-assistant-bg)', borderColor: 'var(--chat-assistant-border)' }}
      >
        {hasReasoning && (
          <Disclosure title="Thinking" icon={<span>✦</span>}>
            <div className="whitespace-pre-wrap text-xs text-muted">{reasoning}</div>
          </Disclosure>
        )}

        {hasTools && (
          <Disclosure title={`${tools!.length} tool ${tools!.length === 1 ? 'call' : 'calls'}`} icon={<span>⚙</span>}>
            <ul className="space-y-1.5">
              {tools!.map((t, i) => (
                <li key={t.id ?? `${t.name}-${i}`} className="flex items-start gap-2 text-xs">
                  <span className="mt-0.5 shrink-0">
                    <ToolStatus status={t.status} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="font-semibold text-fg">{t.name}</span>
                    {t.label && (
                      <span className="mt-0.5 block whitespace-pre-wrap break-words font-[var(--font-mono)] text-muted">
                        {t.label}
                      </span>
                    )}
                  </span>
                </li>
              ))}
            </ul>
          </Disclosure>
        )}

        {content && <Markdown>{resolveAgentMedia(content, agentModel)}</Markdown>}

        {empty && live && (
          <span className="inline-flex gap-1 py-1">
            <Dot /> <Dot delay={0.15} /> <Dot delay={0.3} />
          </span>
        )}
        {content && live && <span className="ml-0.5 inline-block h-4 w-1.5 animate-pulse bg-accent align-middle" />}
        {!live && status === 'streaming' && (
          <div className="text-xs text-muted">· saved (was in progress)</div>
        )}
        {!live && status === 'error' && (
          <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
            {empty
              ? 'The agent returned nothing — its model may not be routable. Check its config and /models.'
              : '· interrupted'}
          </div>
        )}
      </div>
    </div>
  )
}

function ToolStatus({ status }: { status: 'running' | 'completed' }) {
  return status === 'completed' ? (
    <span className="text-[color:var(--theme-success)]">✓</span>
  ) : (
    <span className="inline-block h-2 w-2 animate-pulse rounded-full bg-accent" />
  )
}

function Dot({ delay = 0 }: { delay?: number }) {
  return <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-muted" style={{ animationDelay: `${delay}s` }} />
}

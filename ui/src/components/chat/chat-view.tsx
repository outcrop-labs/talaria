import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { Markdown } from '@/components/ui/markdown'
import { Disclosure } from '@/components/ui/disclosure'
import { streamChat } from '@/lib/chat'
import { mergeTool, type ToolCall } from '@/lib/sse-parse'
import { loadConversation, type StoredMessage } from '@/lib/conversations'

interface DisplayMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning?: string
  tools?: ToolCall[]
  status?: 'streaming' | 'complete' | 'error'
}

const toDisplay = (m: StoredMessage): DisplayMessage => ({
  role: m.role,
  content: m.content,
  reasoning: m.reasoning,
  tools: m.tools,
  status: m.status,
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
}: {
  agentModel: string
  agentLabel: string
  /** Requestable model tiers for this agent (alias names). */
  tiers?: string[]
  conversationId: string | null
  newChatSignal: number
  onCreated: (id: string) => void
}) {
  const [messages, setMessages] = useState<DisplayMessage[]>([])
  const [input, setInput] = useState('')
  const [tier, setTier] = useState('') // '' = the agent's main model
  const [streaming, setStreaming] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const convIdRef = useRef<string | null>(null)
  const bottomRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
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
      if (stop || ++ticks > 90) return clearInterval(iv)
      const res = await loadConversation(id)
      if (!stop && res) setMessages(res.messages.map(toDisplay))
    }, 800)
    return () => {
      stop = true
      clearInterval(iv)
    }
  }, [resuming])

  const send = async () => {
    const text = input.trim()
    if (!text || streaming) return
    setError(null)
    setInput('')
    setMessages((prev) => [
      ...prev,
      { role: 'user', content: text },
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
        { model: agentModel, conversationId: convIdRef.current ?? undefined, content: text, tier: tier || undefined },
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
      }
      patchLast((m) => ({ ...m, status: 'complete' }))
    } catch (e) {
      if ((e as Error).name !== 'AbortError') setError((e as Error).message)
    } finally {
      setStreaming(false)
      abortRef.current = null
    }
  }

  const stop = () => abortRef.current?.abort()
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void send()
    }
  }

  return (
    <div className="mx-auto flex h-full w-full max-w-[var(--chat-content-max-width)] flex-col">
      <div className="flex-1 space-y-5 overflow-y-auto px-6 py-6">
        {messages.length === 0 ? (
          <div className="grid h-full place-items-center text-center">
            <div>
              <div className="mercury-text mb-1 text-lg font-semibold">Talk to {agentLabel}</div>
              <div className="text-sm text-muted">Ask anything — memory, skills, and tools intact.</div>
            </div>
          </div>
        ) : (
          messages.map((m, i) =>
            m.role === 'user' ? (
              <UserBubble key={i} content={m.content} />
            ) : (
              <AssistantTurn key={i} message={m} live={(streaming || resuming) && i === messages.length - 1} />
            ),
          )
        )}
        {error && <div className="text-center text-sm" style={{ color: 'var(--theme-danger)' }}>{error}</div>}
        <div ref={bottomRef} />
      </div>

      <div className="px-6 pb-6">
        <div className="mercury-panel flex items-end gap-2 rounded-2xl p-2">
          <Textarea
            rows={1}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={`Message ${agentLabel}…`}
            className="max-h-40 min-h-[2.75rem] border-0 bg-transparent focus:border-0"
          />
          {tiers.length > 0 && (
            <Select
              value={tier}
              onChange={(e) => setTier(e.target.value)}
              size="sm"
              className="shrink-0 self-center"
              title="Model tier for this turn"
            >
              <option value="">main</option>
              {tiers.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
          )}
          {streaming ? (
            <Button variant="outline" onClick={stop}>Stop</Button>
          ) : (
            <Button onClick={() => void send()} disabled={!input.trim()}>Send</Button>
          )}
        </div>
      </div>
    </div>
  )
}

function UserBubble({ content }: { content: string }) {
  return (
    <div className="flex justify-end">
      <div
        className="max-w-[85%] whitespace-pre-wrap rounded-2xl border px-4 py-2.5 text-sm text-[color:var(--chat-user-foreground)]"
        style={{ background: 'var(--chat-user-bg)', borderColor: 'var(--chat-user-border)' }}
      >
        {content}
      </div>
    </div>
  )
}

function AssistantTurn({ message, live }: { message: DisplayMessage; live: boolean }) {
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

        {content && <Markdown>{content}</Markdown>}

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
          <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>· interrupted</div>
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

import { useEffect, useRef, useState } from 'react'
import { useContextMenu } from '@/components/ui/context-menu'
import { AgentChip } from '@/components/chat/agent-picker'
import { TierPicker } from '@/components/chat/tier-picker'
import { StopButton } from '@/components/chat/composer-buttons'
import { KeyHint } from '@/components/ui/kbd'
import { type Mentionable } from '@/components/chat/mentions'
import { EmojiButton } from '@/components/chat/emoji'
import { ChatComposer, type ChatComposerHandle } from '@/components/chat/chat-composer'
import { MessageAvatar } from '@/components/chat/chat-chrome'
import { AttachButton, PendingAttachments, MessageAttachments } from '@/components/chat/attachments'
import { GuardCaveat, type GuardFinding } from '@/components/chat/guard-caveat'
import { Markdown } from '@/components/ui/markdown'
import { Disclosure } from '@/components/ui/disclosure'
import { GeneratingBars, GeneratingDots, GeneratingHelix } from '@/components/ui/generating'
import { Skeleton } from '@/components/ui/skeleton'
import { resolveAgentMedia } from '@/lib/agent-media'
import { queueChatMessage, streamChat } from '@/lib/chat'
import { mergeTool, type ToolCall } from '@/lib/sse-parse'
import { loadConversation, type StoredMessage } from '@/lib/conversations'
import { uploadFile, splitAttachments, type Attachment } from '@/lib/attachments'

interface DisplayMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning?: string
  tools?: ToolCall[]
  status?: 'streaming' | 'complete' | 'error'
  attachments?: Attachment[]
  /** Who wrote a user turn — shown in multiplayer plans to tell voices apart. */
  authorLabel?: string | null
  /** Confab-guard findings pinned to a reply (annotate/strict modes). */
  guard?: GuardFinding[] | null
}

const toDisplay = (m: StoredMessage): DisplayMessage => ({
  role: m.role,
  content: m.content,
  reasoning: m.reasoning,
  tools: m.tools,
  status: m.status,
  attachments: m.attachments,
  authorLabel: m.authorLabel,
  guard: m.guard,
})

// A durable chat thread. Server owns history; this loads an existing conversation
// (conversationId) or starts fresh (newChatSignal), and streams new turns.
export function ChatView({
  agentModel,
  agentLabel,
  tiers = [],
  agents = [],
  onAgentChange,
  conversationId,
  newChatSignal,
  onCreated,
  kind = 'chat',
  templateId,
  fill = false,
  mentionables = [],
  onTurnComplete,
}: {
  agentModel: string
  agentLabel: string
  /** Requestable model tiers for this agent (alias names). */
  tiers?: string[]
  /** Fleet options for the composer rail's agent chip (spec §7). A
   *  conversation is bound to its agent, so picking a different one is
   *  route-level: `onAgentChange` swaps to that agent's thread — the same
   *  behavior as picking the agent in the host's sidebar. Chip renders only
   *  when both are provided. */
  agents?: { id: string; label: string; role?: string }[]
  onAgentChange?: (id: string) => void
  conversationId: string | null
  newChatSignal: number
  onCreated: (id: string) => void
  /** 'plan' conversations live in the Plan surface and draft tickets. */
  kind?: 'chat' | 'plan'
  /** Plan surface: the template the living doc seeds from, chosen before the
   *  first turn (which creates the conversation). Ignored once it exists. */
  templateId?: string | null
  /** Fill the parent pane instead of centering on the chat width token —
   *  the plan surface's side-by-side split owns its own geometry. */
  fill?: boolean
  /** Composer @mention options (e.g. the plan surface offers teammates). */
  mentionables?: Mentionable[]
  /** Fires each time an agent turn lands complete, whether this client
   *  streamed it or the poller observed a server-chained one (the plan
   *  surface syncs its living document on this). */
  onTurnComplete?: () => void
}) {
  const [messages, setMessages] = useState<DisplayMessage[]>([])
  const [composerEmpty, setComposerEmpty] = useState(true)
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [tier, setTier] = useState('') // '' = the agent's main model
  const [streaming, setStreaming] = useState(false)
  // True while an EXISTING conversation's history is being fetched — the pane
  // shows transcript-shaped skeletons, never the "Talk to X" hero.
  const [loadingConversation, setLoadingConversation] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Right-click a bubble → copy its text (the content is already markdown,
  // so one copy action covers both plain and markdown wants).
  const { openMenu, menu } = useContextMenu()
  const abortRef = useRef<AbortController | null>(null)
  const convIdRef = useRef<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const prevCount = useRef(0)
  const composerRef = useRef<ChatComposerHandle>(null)

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
    setLoadingConversation(false)
    setError(null)
  }, [newChatSignal])

  // Load an existing conversation when the selection changes.
  useEffect(() => {
    if (!conversationId || conversationId === convIdRef.current) return
    abortRef.current?.abort()
    convIdRef.current = conversationId
    let cancelled = false
    setLoadingConversation(true)
    loadConversation(conversationId)
      .then((res) => {
        if (!cancelled) setMessages((res?.messages ?? []).map(toDisplay))
      })
      .finally(() => {
        if (!cancelled) setLoadingConversation(false)
      })
    return () => {
      cancelled = true
    }
  }, [conversationId])

  // Live-resume: poll the persisted state whenever the server owes us words we
  // didn't start streaming ourselves — a reload landed mid-generation (last
  // reply still 'streaming'), or the last message is the user's (a queued
  // message whose chained follow-up turn hasn't appeared yet — without this
  // the follow-up lands server-side but the chat never shows it). Capped so
  // it can't poll forever.
  const last = messages[messages.length - 1]
  const resuming = !streaming && (last?.role === 'user' || (last?.role === 'assistant' && last.status === 'streaming'))

  // Turn-landing edge: fire onTurnComplete when an IN-FLIGHT turn (one we
  // streamed, or one the poller was watching) flips to a complete assistant
  // reply. The ref arms only while something is in flight, so loading an old
  // conversation never fires it.
  const turnInFlight = useRef(false)
  const onTurnCompleteRef = useRef(onTurnComplete)
  onTurnCompleteRef.current = onTurnComplete
  useEffect(() => {
    const landed = last?.role === 'assistant' && last.status === 'complete'
    if ((streaming || resuming) && !landed) turnInFlight.current = true
    else if (landed && turnInFlight.current) {
      turnInFlight.current = false
      onTurnCompleteRef.current?.()
    }
  }, [last, streaming, resuming])
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

  const send = async (text: string) => {
    if (!text && attachments.length === 0) return
    const atts = attachments
    setError(null)
    setAttachments([])
    composerRef.current?.clear()

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
          ...splitAttachments(atts),
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
        { model: agentModel, conversationId: convIdRef.current ?? undefined, content: text, tier: tier || undefined, ...splitAttachments(atts), kind, templateId },
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


  // Esc stops the stream even when focus wandered off the textarea.
  useEffect(() => {
    if (!streaming) return
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') stop()
    }
    window.addEventListener('keydown', onEsc)
    return () => window.removeEventListener('keydown', onEsc)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streaming])

  return (
    <div className={fill ? 'flex h-full w-full flex-col' : 'mx-auto flex h-full w-full max-w-[var(--chat-content-max-width)] flex-col'}>
      <div ref={scrollRef} className="flex-1 space-y-5 overflow-y-auto px-6 py-6">
        {loadingConversation ? (
          // Conversation-shaped shimmer while the thread's history loads —
          // flattened message rows (avatar square + name + body), never the
          // "Talk to X" hero.
          <div aria-hidden className="space-y-5">
            {Array.from({ length: 5 }, (_, i) => (
              <div key={i} className="flex gap-2.5">
                <Skeleton className="mt-0.5 h-6 w-6 shrink-0 rounded" delay={i * 0.12} />
                <div className="min-w-0 flex-1 space-y-2 pt-1">
                  <Skeleton className="h-2.5 w-24 rounded-full" delay={i * 0.12} />
                  <div style={{ width: ['62%', '84%', '48%', '90%', '70%'][i] }}>
                    <Skeleton className="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.06} />
                  </div>
                </div>
              </div>
            ))}
          </div>
        ) : messages.length === 0 ? (
          <div className="grid h-full place-items-center text-center">
            <div>
              <div className="mb-1 font-sans text-lg font-semibold text-fg">
                {kind === 'plan' ? `Plan with ${agentLabel}` : `Talk to ${agentLabel}`}
              </div>
              <div className="font-sans text-sm text-muted">
                {kind === 'plan'
                  ? 'Think through the work together, then draft tickets and send them to a board.'
                  : 'Ask anything. Memory, skills, and tools intact.'}
              </div>
            </div>
          </div>
        ) : (
          messages.map((m, i) => {
            const copyMenu = (e: React.MouseEvent) =>
              openMenu(e, [
                { label: 'Copy text', disabled: !m.content, onSelect: () => void navigator.clipboard.writeText(m.content) },
              ])
            return m.role === 'user' ? (
              // Flattened user turn (spec §10) — the author name keeps the
              // multiplayer voices apart on shared plans.
              <UserTurn
                key={i}
                content={m.content}
                attachments={m.attachments}
                author={m.authorLabel ?? null}
                onContextMenu={copyMenu}
              />
            ) : (
              <AssistantTurn
                key={i}
                message={m}
                agentModel={agentModel}
                agentLabel={agentLabel}
                live={(streaming || resuming) && i === messages.length - 1}
                onContextMenu={copyMenu}
              />
            )
          })
        )}
        {error && <div className="text-center text-sm" style={{ color: 'var(--theme-danger)' }}>{error}</div>}
      </div>

      <div className="relative px-6 pb-6">
        {/* The composer panel (spec §7): #141312 body, strong 1px border,
            radius 8, 8px padding/gap, matte float shadow. */}
        <div
          className="flex flex-col gap-2 rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]"
          onDragOver={(e) => {
            if (e.dataTransfer.types.includes('Files')) e.preventDefault()
          }}
          onDrop={(e) => {
            const files = Array.from(e.dataTransfer?.files ?? [])
            if (files.length === 0) return
            e.preventDefault()
            for (const f of files) {
              void uploadFile(f).then((r) => {
                if ('id' in r) setAttachments((prev) => [...prev, r])
              })
            }
          }}
        >
          <PendingAttachments items={attachments} onRemove={(id) => setAttachments((prev) => prev.filter((a) => a.id !== id))} />
          <ChatComposer
            ref={composerRef}
            placeholder={`What would you like ${agentLabel} to work on?`}
            mentionables={mentionables}
            onSubmit={(md) => void send(md)}
            onFiles={(files) => {
              for (const f of files) {
                void uploadFile(f).then((r) => {
                  if ('id' in r) setAttachments((prev) => [...prev, r])
                })
              }
            }}
            onEscape={streaming ? stop : undefined}
            onEmptyChange={setComposerEmpty}
            canSend={!composerEmpty || attachments.length > 0}
            leftControls={
              <>
                <AttachButton onAttach={(a) => setAttachments((prev) => [...prev, a])} disabled={streaming} />
                <EmojiButton onPick={(ch) => composerRef.current?.insertText(ch)} />
              </>
            }
            rightControls={
              <>
                {/* Spec §7 rail order: agent chip, then model chip. */}
                {agents.length > 0 && onAgentChange && (
                  <AgentChip agents={agents} value={agentModel} onChange={onAgentChange} />
                )}
                {tiers.length > 0 && <TierPicker tiers={tiers} value={tier} onChange={setTier} />}
                {streaming && <StopButton onClick={stop} />}
                <KeyHint
                  keys={streaming ? 'esc' : '⏎'}
                  label={streaming ? 'stop' : 'send'}
                  visible={streaming || !composerEmpty || attachments.length > 0}
                />
              </>
            }
          />
        </div>
      </div>
      {menu}
    </div>
  )
}

/** Flattened user turn (spec §10): avatar square + name row, 14px sans body —
 *  no heavy bubble. */
function UserTurn({
  content,
  attachments,
  author,
  onContextMenu,
}: {
  content: string
  attachments?: Attachment[]
  author?: string | null
  onContextMenu?: (e: React.MouseEvent) => void
}) {
  const name = author ?? 'You'
  return (
    <div className="flex gap-2.5" onContextMenu={onContextMenu}>
      <MessageAvatar name={name} className="mt-0.5" />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="font-sans text-[13px] font-medium text-fg">{name}</span>
        </div>
        <div className="font-sans text-sm text-fg">
          {/* Markdown, like every other message surface — a user turn was the
              one bubble still rendering raw text. */}
          <Markdown>{content}</Markdown>
          {attachments && attachments.length > 0 && <MessageAttachments items={attachments} />}
        </div>
      </div>
    </div>
  )
}

/** Flattened assistant turn (spec §10): avatar square + agent name, compact
 *  mono tool rows, 14px sans body. */
function AssistantTurn({
  message,
  agentModel,
  agentLabel,
  live,
  onContextMenu,
}: {
  message: DisplayMessage
  agentModel: string
  agentLabel: string
  live: boolean
  onContextMenu?: (e: React.MouseEvent) => void
}) {
  const { content, reasoning, tools, status, guard } = message
  const hasReasoning = !!reasoning?.trim()
  const hasTools = !!tools?.length
  const empty = !content && !hasReasoning && !hasTools

  return (
    <div className="flex gap-2.5" onContextMenu={onContextMenu}>
      <MessageAvatar name={agentLabel} className="mt-0.5" />
      <div className="min-w-0 flex-1 space-y-2">
        <div className="flex items-baseline gap-2">
          <span className="font-sans text-[13px] font-medium text-fg">{agentLabel}</span>
          <span className="rounded border border-line px-1 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">
            agent
          </span>
        </div>

        {hasReasoning && (
          <Disclosure title="Thinking" icon={<span>✦</span>}>
            <div className="whitespace-pre-wrap text-xs text-muted">{reasoning}</div>
          </Disclosure>
        )}

        {hasTools && (
          <Disclosure title={`${tools!.length} tool ${tools!.length === 1 ? 'call' : 'calls'}`} icon={<span>⚙</span>}>
            {/* Compact mono tool rows, hairline separated (spec §10). */}
            <ul className="divide-y divide-line">
              {tools!.map((t, i) => (
                <li key={t.id ?? `${t.name}-${i}`} className="flex items-start gap-2 py-1.5 font-mono text-[11px] first:pt-0 last:pb-0">
                  <span className="mt-0.5 shrink-0">
                    <ToolStatus status={t.status} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="text-fg">{t.name}</span>
                    {t.label && (
                      <span className="mt-0.5 block whitespace-pre-wrap break-words text-muted">{t.label}</span>
                    )}
                  </span>
                </li>
              ))}
            </ul>
          </Disclosure>
        )}

        {content && (
          <div className="font-sans text-sm text-fg">
            <Markdown>{resolveAgentMedia(content, agentModel)}</Markdown>
          </div>
        )}
        {!live && <GuardCaveat findings={guard} />}

        {/* Spec §9 state mapping: submitting (awaiting the first token) rides
            the fast gold SIGNAL WEAVE; once reasoning/tools stream but prose
            hasn't landed, the CONTEXT HELIX loops on the standard budget. */}
        {empty && live && <GeneratingDots className="py-1" />}
        {!empty && !content && live && <GeneratingHelix className="py-1" />}
        {content && live && <span className="gd-pulse ml-0.5 inline-block h-4 w-1.5 bg-accent align-middle" />}
        {!live && status === 'streaming' && (
          <div className="font-mono text-[11px] text-muted">· saved (was in progress)</div>
        )}
        {!live && status === 'error' && (
          <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
            {empty
              ? 'The agent returned nothing. Its model may not be routable; check its config and /models.'
              : '· interrupted'}
          </div>
        )}
      </div>
    </div>
  )
}

function ToolStatus({ status }: { status: 'running' | 'completed' }) {
  // Tool in flight = STAGE SCAN on the standard budget (spec §9), scaled to
  // the 11px mono row; done = the quiet green check.
  return status === 'completed' ? (
    <span className="text-[color:var(--theme-success)]">✓</span>
  ) : (
    <GeneratingBars bars={4} variant="scan" step={0.18} className="text-accent [&>span]:h-[9px]" />
  )
}

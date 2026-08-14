<script lang="ts">
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import AgentChip from '@/components/chat/AgentChip.svelte'
  import TierPicker from '@/components/chat/TierPicker.svelte'
  import StopButton from '@/components/chat/StopButton.svelte'
  import KeyHint from '@/components/ui/KeyHint.svelte'
  import { type Mentionable } from '@/components/chat/mentions.svelte'
  import EmojiButton from '@/components/chat/EmojiButton.svelte'
  import ChatComposer from '@/components/chat/ChatComposer.svelte'
  import type { ChatComposerHandle } from '@/components/chat/chat-composer'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import RelayButton from '@/components/chat/RelayButton.svelte'
  import PendingAttachments from '@/components/chat/PendingAttachments.svelte'
  import UserTurn from './UserTurn.svelte'
  import AssistantTurn from './AssistantTurn.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { slide } from '@/lib/motion'
  import { queueChatMessage, streamChat } from '@/lib/chat'
  import { mergeTool } from '@/lib/sse-parse'
  import { loadConversation } from '@/lib/conversations.svelte'
  import { uploadFile, splitAttachments, type Attachment } from '@/lib/attachments'
  import { toDisplay, type DisplayMessage } from './chat-view'

  // A durable chat thread. Server owns history; this loads an existing conversation
  // (conversationId) or starts fresh (newChatSignal), and streams new turns.
  let {
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
    /** 'plan' conversations live in the Plan surface and draft tickets.
     *  'research' conversations discuss a report on the Research surface — the
     *  same multiplayer shape, shared through the RUN's members. */
    kind?: 'chat' | 'plan' | 'research'
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
  } = $props()

  let messages = $state<DisplayMessage[]>([])
  let composerEmpty = $state(true)
  let attachments = $state<Attachment[]>([])
  let tier = $state('') // '' = the agent's main model
  let streaming = $state(false)
  // True while an EXISTING conversation's history is being fetched — the pane
  // shows transcript-shaped skeletons, never the "Talk to X" hero.
  let loadingConversation = $state(false)
  let error = $state<string | null>(null)
  // Right-click a bubble → copy its text (the content is already markdown,
  // so one copy action covers both plain and markdown wants).
  const menu = useContextMenu()
  let abortCtrl: AbortController | null = null
  let convId: string | null = null
  let scrollEl = $state<HTMLDivElement | null>(null)
  let prevCount = 0
  let composer = $state<ChatComposerHandle | null>(null)

  // Follow the stream WITHOUT smooth-scrolling: token flushes fire this every
  // few ms, and overlapping smooth animations rubber-band (the "bounce").
  // Instant jumps, and only while pinned near the bottom — scrolling up to
  // read history is never yanked away. A fresh load always lands at the end.
  $effect(() => {
    // Snapshot to track every message field, not just the array shape — token
    // flushes mutate the last message in place and must keep the pin.
    void $state.snapshot(messages)
    const el = scrollEl
    if (!el) return
    const loaded = prevCount === 0 && messages.length > 0
    prevCount = messages.length
    const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (loaded || pinned) el.scrollTop = el.scrollHeight
  })
  $effect(() => () => abortCtrl?.abort())

  // New-chat reset (declared before the loader so mount order is reset→load).
  $effect(() => {
    void newChatSignal
    abortCtrl?.abort()
    convId = null
    messages = []
    loadingConversation = false
    error = null
  })

  // Load an existing conversation when the selection changes.
  $effect(() => {
    if (!conversationId || conversationId === convId) return
    abortCtrl?.abort()
    convId = conversationId
    let cancelled = false
    loadingConversation = true
    loadConversation(conversationId)
      .then((res) => {
        if (!cancelled) messages = (res?.messages ?? []).map(toDisplay)
      })
      .finally(() => {
        if (!cancelled) loadingConversation = false
      })
    return () => {
      cancelled = true
    }
  })

  // Live-resume: poll the persisted state whenever the server owes us words we
  // didn't start streaming ourselves — a reload landed mid-generation (last
  // reply still 'streaming'), or the last message is the user's (a queued
  // message whose chained follow-up turn hasn't appeared yet — without this
  // the follow-up lands server-side but the chat never shows it). Capped so
  // it can't poll forever.
  const last = $derived(messages[messages.length - 1])
  const resuming = $derived(
    !streaming && (last?.role === 'user' || (last?.role === 'assistant' && last.status === 'streaming')),
  )

  // Turn-landing edge: fire onTurnComplete when an IN-FLIGHT turn (one we
  // streamed, or one the poller was watching) flips to a complete assistant
  // reply. The flag arms only while something is in flight, so loading an old
  // conversation never fires it. (Props are live in runes mode, so no
  // onTurnCompleteRef is needed to see the fresh callback.)
  let turnInFlight = false
  $effect(() => {
    const landed = last?.role === 'assistant' && last.status === 'complete'
    if ((streaming || resuming) && !landed) turnInFlight = true
    else if (landed && turnInFlight) {
      turnInFlight = false
      onTurnComplete?.()
    }
  })
  $effect(() => {
    if (!resuming) return
    const id = convId
    if (!id) return
    let stop = false
    let ticks = 0
    const iv = setInterval(async () => {
      if (stop || ++ticks > 300) return clearInterval(iv) // ~4 min — long agent replies keep animating
      const res = await loadConversation(id)
      if (!stop && res) messages = res.messages.map(toDisplay)
    }, 800)
    return () => {
      stop = true
      clearInterval(iv)
    }
  })

  // Refresh from the server's truth: queued messages, and any follow-up turn
  // the server chained (the resuming poller then animates it live).
  const syncFromServer = async () => {
    const id = convId
    if (!id) return
    const res = await loadConversation(id)
    if (res && convId === id) messages = res.messages.map(toDisplay)
  }

  const send = async (text: string) => {
    if (!text && attachments.length === 0) return
    const atts = attachments
    error = null
    attachments = []
    composer?.clear()

    // Claude-style flow: sending while the agent is replying never interrupts —
    // the message queues into history and the agent picks it up next turn.
    if (streaming && convId) {
      messages.push({ role: 'user', content: text, attachments: atts })
      try {
        await queueChatMessage({
          model: agentModel,
          conversationId: convId,
          content: text,
          tier: tier || undefined,
          ...splitAttachments(atts),
          kind,
        })
      } catch (e) {
        error = (e as Error).message
      }
      return
    }

    messages.push(
      { role: 'user', content: text, attachments: atts },
      { role: 'assistant', content: '', reasoning: '', tools: [], status: 'streaming' },
    )
    streaming = true

    const patchLast = (fn: (m: DisplayMessage) => DisplayMessage) => {
      const l = messages[messages.length - 1]
      if (l?.role === 'assistant') messages[messages.length - 1] = fn(l)
    }

    const ctrl = new AbortController()
    abortCtrl = ctrl
    try {
      for await (const ev of streamChat(
        { model: agentModel, conversationId: convId ?? undefined, content: text, tier: tier || undefined, ...splitAttachments(atts), kind, templateId },
        (meta) => {
          if (!convId) {
            convId = meta.conversationId
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
          if (messages[messages.length - 1]?.role === 'assistant') messages.pop()
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
      if ((e as Error).name !== 'AbortError') error = (e as Error).message
    } finally {
      streaming = false
      abortCtrl = null
      // Pick up whatever happened meanwhile: queued messages, and the
      // follow-up turn the server chains for them (the resuming poller
      // animates it live once it appears).
      void syncFromServer()
    }
  }

  const stop = () => abortCtrl?.abort()

  // Esc stops the stream even when focus wandered off the textarea.
  $effect(() => {
    if (!streaming) return
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') stop()
    }
    window.addEventListener('keydown', onEsc)
    return () => window.removeEventListener('keydown', onEsc)
  })

  const copyMenu = (m: DisplayMessage) => (e: MouseEvent) =>
    menu.openMenu(e, [
      { label: 'Copy text', disabled: !m.content, onSelect: () => void navigator.clipboard.writeText(m.content) },
    ])
</script>

<div class={fill ? 'flex h-full w-full flex-col' : 'mx-auto flex h-full w-full max-w-[var(--chat-content-max-width)] flex-col'}>
  <div bind:this={scrollEl} class="flex-1 space-y-5 overflow-y-auto px-6 py-6">
    {#if loadingConversation}
      <!-- Conversation-shaped shimmer while the thread's history loads —
          flattened message rows (avatar square + name + body), never the
          "Talk to X" hero. -->
      <div aria-hidden="true" class="space-y-5">
        {#each Array.from({ length: 5 }) as _, i (i)}
          <div class="flex gap-2.5">
            <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded" delay={i * 0.12} />
            <div class="min-w-0 flex-1 space-y-2 pt-1">
              <Skeleton class="h-2.5 w-24 rounded-full" delay={i * 0.12} />
              <div style:width={['62%', '84%', '48%', '90%', '70%'][i]}>
                <Skeleton class="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.06} />
              </div>
            </div>
          </div>
        {/each}
      </div>
    {:else if messages.length === 0}
      <div class="grid h-full place-items-center text-center">
        <div>
          <div class="mb-1 font-sans text-lg font-semibold text-fg">
            {kind === 'plan' ? `Plan with ${agentLabel}` : `Talk to ${agentLabel}`}
          </div>
          <div class="font-sans text-sm text-muted">
            {kind === 'plan'
              ? 'Think through the work together, then draft tickets and send them to a board.'
              : 'Ask anything. Memory, skills, and tools intact.'}
          </div>
        </div>
      </div>
    {:else}
      {#each messages as m, i (i)}
        {#if m.role === 'user'}
          <!-- Flattened user turn (spec §10) — the author name keeps the
              multiplayer voices apart on shared plans. -->
          <UserTurn content={m.content} attachments={m.attachments} author={m.authorLabel ?? null} onContextMenu={copyMenu(m)} />
        {:else}
          <AssistantTurn
            message={m}
            {agentModel}
            {agentLabel}
            live={(streaming || resuming) && i === messages.length - 1}
            onContextMenu={copyMenu(m)}
          />
        {/if}
      {/each}
    {/if}
    {#if error}
      <div transition:slide={{ duration: 150 }} class="text-center text-sm" style:color="var(--theme-danger)">{error}</div>
    {/if}
  </div>

  <div class="relative px-6 pb-6">
    <!-- The composer panel (spec §7): #141312 body, strong 1px border,
        radius 8, 8px padding/gap, matte float shadow. -->
    <div
      class="flex flex-col gap-2 rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]"
      ondragover={(e) => {
        if (e.dataTransfer?.types.includes('Files')) e.preventDefault()
      }}
      ondrop={(e) => {
        const files = Array.from(e.dataTransfer?.files ?? [])
        if (files.length === 0) return
        e.preventDefault()
        for (const f of files) {
          void uploadFile(f).then((r) => {
            if ('id' in r) attachments.push(r)
          })
        }
      }}
      role="group"
    >
      <PendingAttachments items={attachments} onRemove={(id) => (attachments = attachments.filter((a) => a.id !== id))} />
      <ChatComposer
        bind:this={composer}
        placeholder={`What would you like ${agentLabel} to work on?`}
        {mentionables}
        onSubmit={(md) => void send(md)}
        onFiles={(files) => {
          for (const f of files) {
            void uploadFile(f).then((r) => {
              if ('id' in r) attachments.push(r)
            })
          }
        }}
        onEscape={streaming ? stop : undefined}
        onEmptyChange={(v) => (composerEmpty = v)}
        canSend={!composerEmpty || attachments.length > 0}
      >
        {#snippet leftControls()}
          <AttachButton onAttach={(a) => attachments.push(a)} disabled={streaming} />
          <EmojiButton onPick={(ch) => composer?.insertText(ch)} />
          <!-- The handle lands in the editor; the value never does. See
               RelayButton.svelte for why that is a property of the route it
               took rather than a rule anybody has to remember. -->
          <RelayButton {agentModel} {agentLabel} onMinted={(h) => composer?.insertText(h)} disabled={streaming} />
        {/snippet}
        {#snippet rightControls()}
          <!-- Spec §7 rail order: agent chip, then model chip. -->
          {#if agents.length > 0 && onAgentChange}
            <AgentChip {agents} value={agentModel} onChange={onAgentChange} />
          {/if}
          {#if tiers.length > 0}<TierPicker {tiers} value={tier} onChange={(t) => (tier = t)} />{/if}
          {#if streaming}<StopButton onClick={stop} />{/if}
          <KeyHint
            keys={streaming ? 'esc' : '⏎'}
            label={streaming ? 'stop' : 'send'}
            visible={streaming || !composerEmpty || attachments.length > 0}
          />
        {/snippet}
      </ChatComposer>
    </div>
  </div>
  <ContextMenu {menu} />
</div>

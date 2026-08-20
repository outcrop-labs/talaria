<script lang="ts">
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import AgentChip from '@/components/chat/AgentChip.svelte'
  import TierPicker from '@/components/chat/TierPicker.svelte'
  import EffortPicker from '@/components/chat/EffortPicker.svelte'
  import StopButton from '@/components/chat/StopButton.svelte'
  import KeyHint from '@/components/ui/KeyHint.svelte'
  import { type Mentionable } from '@/components/chat/mentions.svelte'
  import EmojiButton from '@/components/chat/EmojiButton.svelte'
  import { bottomStick } from '@/lib/stick-to-bottom'
  import ChatComposer from '@/components/chat/ChatComposer.svelte'
  import type { ChatComposerHandle } from '@/components/chat/chat-composer'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import RelayButton from '@/components/chat/RelayButton.svelte'
  import PendingAttachments from '@/components/chat/PendingAttachments.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'
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
    mentionables = [],
    onTurnComplete,
    minimal = false,
    tier = $bindable(''),
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
    /** Composer @mention options (e.g. the plan surface offers teammates). */
    mentionables?: Mentionable[]
    /** Fires each time an agent turn lands complete, whether this client
     *  streamed it or the poller observed a server-chained one (the plan
     *  surface syncs its living document on this). */
    onTurnComplete?: () => void
    /** The view OWNS the conversation partner: the surface's sidebar picks the
     *  agent, its chrome picks the harness/model. So the composer rail drops
     *  both pickers and the relay/emoji affordances — the surface is left with
     *  exactly attach, text, and submit (plus stop while a reply streams). */
    minimal?: boolean
    /** Requestable model tier for this conversation ('' = the agent's main
     *  model). Bindable so a surface can lift the pick up into its own chrome
     *  (the Plan surface renders it in the stage header). */
    tier?: string
  } = $props()

  let messages = $state<DisplayMessage[]>([])
  let composerEmpty = $state(true)
  /** Measured height of the floating composer — what the transcript reserves. */
  let composerH = $state(0)
  let attachments = $state<Attachment[]>([])
  let streaming = $state(false)
  // The effort pick for this conversation's next turns ('' = model default).
  // Offered only when the ROUTED model's metadata vouches for levels — a tier
  // switch can change the list, and a pick that leaves it resets to default.
  let effort = $state('')
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
  let composer = $state<ChatComposerHandle | null>(null)

  // Follow the stream WITHOUT smooth-scrolling: token flushes fire this every
  // few ms, and overlapping smooth animations rubber-band (the "bounce").
  // Follow the newest turn unless the reader scrolled away (lib/stick-to-bottom
  // — and see its header for the pin bug all three surfaces used to share).
  const stick = bottomStick()
  $effect(() => stick.attach(scrollEl))
  $effect(() => {
    // Snapshot to track every message FIELD, not just the array shape — token
    // flushes mutate the last message in place and still have to follow.
    void $state.snapshot(messages)
    stick.follow()
  })
  // A different conversation opens at its own newest message, whatever the
  // reader happened to be doing in the last one.
  $effect(() => {
    void conversationId
    void newChatSignal
    stick.jump()
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

  // ── Effort offering ────────────────────────────────────────────────────────
  // The routed id is what the server validates against, so it is also what the
  // picker lists from — `routedModelFor` builds exactly this string when the
  // tier is one of the agent's aliases.
  const routedModel = $derived(tier ? `${agentModel}-${tier}` : agentModel)
  const { efforts } = useModelEfforts(() => routedModel)
  // A model switch (agent or tier) can retire the picked level; reset to the
  // default rather than sending a level the new model would 400 on.
  $effect(() => {
    if (effort && !efforts.includes(effort)) effort = ''
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
    // You always see what you just said, even if you were reading history.
    stick.jump()
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
          effort: effort || undefined,
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
        { model: agentModel, conversationId: convId ?? undefined, content: text, tier: tier || undefined, effort: effort || undefined, ...splitAttachments(atts), kind, templateId },
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

<!-- The transcript owns the whole surface and the composer FLOATS over it.
     The reading measure did not go away — it moved inward, from this root onto
     the message column, which is the difference between "the chat is a 900px
     card sitting in the middle of the stage" and "the chat is the stage, and
     its text is set to a readable measure". The zero state and the scrollbar
     now reach the edges; the words still don't.

     `--chat-composer` is the measured height of the float, reserved at the
     bottom of the scroll so the last message never parks behind it. Measured,
     not a constant: the composer grows with the draft. -->
<div class="relative flex h-full w-full flex-col" style:--chat-composer="{composerH}px">
  <div
    bind:this={scrollEl}
    class="flex-1 overflow-y-auto"
  >
    {#if loadingConversation}
      <!-- Conversation-shaped shimmer while the thread's history loads —
          flattened message rows (avatar square + name + body), never the
          "Talk to X" hero. -->
      <div aria-hidden="true" class="mx-auto w-full max-w-[var(--converse-width)] space-y-5 px-6 py-6">
        {#each Array.from({ length: 5 }) as _, i (i)}
          <div class="flex gap-2.5">
            <Skeleton class="mt-0.5 h-6 w-6 shrink-0 rounded" />
            <div class="min-w-0 flex-1 space-y-2 pt-1">
              <Skeleton class="h-2.5 w-24 rounded-full" />
              <div style:width={['62%', '84%', '48%', '90%', '70%'][i]}>
                <Skeleton class="h-2.5 w-full rounded-full" />
              </div>
            </div>
          </div>
        {/each}
      </div>
    {:else if messages.length === 0}
      <!-- Outside the measure on purpose: this centres in the whole surface,
           where before it centred inside a 900px column that was itself
           centred — off-centre twice over on a wide stage. -->
      <div class="grid h-full place-items-center px-6 py-6 text-center">
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
      <!-- The clearance is on the MESSAGE LIST, not on this scroll box. On the
           box it applied to every branch — so the transcript stopped short of
           the composer instead of running under it, and the zero state was
           held off the bottom of its own void. Only content that can be read
           needs to clear the float; the surface behind it should not. -->
      <div
        class="mx-auto w-full max-w-[var(--converse-width)] space-y-5 px-6 py-6"
        style:padding-bottom="calc(var(--chat-composer, 0px) + 0.5rem)"
      >
      {#each messages as m, i (i)}
        {#if m.role === 'user'}
          <!-- Flattened user turn (spec §10) — the author name keeps the
              multiplayer voices apart on shared plans. -->
          <UserTurn content={m.content} attachments={m.attachments} author={m.authorLabel ?? null} onContextMenu={copyMenu(m)} />
        {:else}
          <AssistantTurn
            message={m}
            turn={i}
            {agentModel}
            {agentLabel}
            live={(streaming || resuming) && i === messages.length - 1}
            onContextMenu={copyMenu(m)}
          />
        {/if}
      {/each}
      </div>
    {/if}
    {#if error}
      <div class="mx-auto w-full max-w-[var(--converse-width)] px-6 pb-6">
        <div transition:slide={{ duration: 150 }} class="text-center text-sm" style:color="var(--theme-danger)">{error}</div>
      </div>
    {/if}
  </div>

  <!-- pointer-events-none on the gutter, auto on the panel: the float spans
       the surface so the panel can centre in it, and without that split the
       transparent margin either side would swallow clicks on the transcript. -->
  <!-- The gutter goes INSIDE the measure, not outside it. With `px-6` on this
       outer element the panel centred at the full 900px (598→1498) while the
       message rows sat inside their own `px-6` (622→1474) — so the composer
       overhung the conversation by 24px a side. Same wrapper shape as the
       message list now, so the two columns line up by construction.
       The band is OPAQUE. The float's gutter was transparent, so the transcript
       scrolled visibly through the strip below the composer panel — text
       peeking out under it, which is what read as broken. `bg-surface` is the
       ground the stage already paints (#090a09), so the composer sits on the
       page rather than on a patch. A gradient scrim would fade the clip more
       softly, but this design system has no gradients anywhere and the matte
       rule is the house style. -->
  <div bind:clientHeight={composerH} class="pointer-events-none absolute inset-x-0 bottom-0 z-10 bg-surface pb-6">
    <div class="mx-auto w-full max-w-[var(--converse-width)] px-6">
    <!-- The composer panel (spec §7): #141312 body, strong 1px border,
        radius 8, 8px padding/gap, matte float shadow. -->
    <div
      class="pointer-events-auto flex flex-col gap-2 rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]"
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
          {#if !minimal}
            <EmojiButton onPick={(ch) => composer?.insertText(ch)} />
            <!-- The handle lands in the editor; the value never does. See
                 RelayButton.svelte for why that is a property of the route it
                 took rather than a rule anybody has to remember. -->
            <RelayButton {agentModel} {agentLabel} onMinted={(h) => composer?.insertText(h)} disabled={streaming} />
          {/if}
        {/snippet}
        {#snippet rightControls()}
          <!-- Spec §7 rail order: agent chip, then model chip, then stop.
                MINIMAL mode (plan/research): the surface owns the agent in its
                sidebar and the model in its chrome, so both pickers stay out of
                the composer — send is the last word on the rail. -->
          {#if !minimal}
            {#if agents.length > 0 && onAgentChange}
              <AgentChip {agents} value={agentModel} onChange={onAgentChange} />
            {/if}
            {#if tiers.length > 0}<TierPicker {tiers} value={tier} onChange={(t) => (tier = t)} />{/if}
          {/if}
          {#if streaming}<StopButton onClick={stop} />{/if}
          <!-- MINIMAL mode (plan/research) leaves the KeyHint out too: the
                surface pane is narrow, the hint's always-rendered slot was what
                pushed the send tile past the panel's edge, and Esc/Enter behave
                identically here as in comms. -->
          {#if !minimal}
            <KeyHint
              keys={streaming ? 'esc' : '⏎'}
              label={streaming ? 'stop' : 'send'}
              visible={streaming || !composerEmpty || attachments.length > 0}
            />
            <!-- Effort sits immediately left of the send tile, and only when the
                 routed model's metadata vouches for levels — a model with no
                 published ladder shows no chip and its requests carry no effort.
                 Not disabled while streaming (TierPicker isn't either): a
                 queued message picks up the level set when it is sent. -->
            {#if efforts.length > 0}<EffortPicker {efforts} value={effort} onChange={(v) => (effort = v)} />{/if}
          {/if}
        {/snippet}
      </ChatComposer>
      </div>
    </div>
  </div>
  <ContextMenu {menu} />
</div>

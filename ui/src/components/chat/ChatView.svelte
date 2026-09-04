<script lang="ts">
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import TierPicker from '@/components/chat/TierPicker.svelte'
  import EffortPicker from '@/components/chat/EffortPicker.svelte'
  import { type Mentionable } from '@/components/chat/mentions.svelte'
  import EmojiButton from '@/components/chat/EmojiButton.svelte'
  import { bottomStick } from '@/lib/stick-to-bottom'
  import ChatComposer from '@/components/chat/ChatComposer.svelte'
  import type { ChatComposerHandle } from '@/components/chat/chat-composer'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import RelayButton from '@/components/chat/RelayButton.svelte'
  import PendingAttachments from '@/components/chat/PendingAttachments.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'
  import { useProfilePrefs } from '@/lib/muse.svelte'
  import UserTurn from './UserTurn.svelte'
  import AssistantTurn from './AssistantTurn.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { slide } from '@/lib/motion'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { queueChatMessage, streamChat } from '@/lib/chat'
  import { mergeTool } from '@/lib/sse-parse'
  import { loadConversation, markConversationRead } from '@/lib/conversations.svelte'
  import { uploadFile, splitAttachments, type Attachment } from '@/lib/attachments'
  import { toDisplay, type DisplayMessage } from './chat-view'

  // A durable chat thread. Server owns history; this loads an existing conversation
  // (conversationId) or starts fresh (newChatSignal), and streams new turns.
  let {
    agentModel,
    agentLabel,
    tiers = [],
    conversationId,
    newChatSignal,
    syncSignal = 0,
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
    conversationId: string | null
    newChatSignal: number
    /** An inbound nudge to re-read history — the surface knows about turns this
     *  view's own pollers will not see (the Research run posts its scope
     *  questions and report-ready turn straight into the conversation). */
    syncSignal?: number
    onCreated: (id: string) => void
    /** 'plan' conversations live in the Plan surface and draft tickets.
     *  'research' conversations discuss a report on the Research surface — the
     *  same multiplayer shape, shared through the RUN's members. 'ticket'
     *  conversations are a board ticket's discussion thread: the room is the
     *  board, and the thread only ever exists — this view never creates one. */
    kind?: 'chat' | 'plan' | 'research' | 'ticket'
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
     *  the relay/emoji affordances — the surface is left with exactly attach,
     *  text, and submit (plus stop while a reply streams). */
    minimal?: boolean
    /** Requestable model tier for this conversation ('' = the agent's main
     *  model). Bindable so a surface can lift the pick up into its own chrome
     *  (the Plan surface renders it in the stage header). */
    tier?: string
  } = $props()

  let messages = $state<DisplayMessage[]>([])
  const qc = useQueryClient()
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

  // ── The message queue ──────────────────────────────────────────────────────
  // Claude-style: sending while the agent replies NEVER interrupts. The server
  // has always had the queue (POST /api/chat `queue: true` inserts the message
  // and the completing turn chains the next one), but the client had a hole in
  // it: a message sent during the FIRST turn's opening window — after the
  // request leaves but before the response headers carry the conversation id
  // back, which the server can hold for minutes behind a restarting agent —
  // took the fresh-send path and started a SECOND stream, forking the thread
  // and visibly interrupting the reply. Those messages are now held locally
  // and flushed the moment the id lands; if the first turn dies without ever
  // producing one, they are re-sent as a fresh turn instead of vanishing.
  let held: Array<{ text: string; atts: Attachment[] }> = []
  // Set when the READER stopped a turn (the send tile's stop face / Esc). The
  // server still finishes and persists its copy — history is the server's, by
  // design — but the transcript freezes what was on screen and the
  // live-resume poller must not re-animate the reply the reader walked away
  // from. Without this, "stop" read as a stutter: the text kept arriving via
  // the poller until the server finished anyway.
  let userStopped = $state(false)

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
    // A hold belongs to the thread it was typed into; a fresh thread starts
    // owed nothing.
    held = []
    userStopped = false
    loadingConversation = false
    error = null
  })

  // ── Effort offering ────────────────────────────────────────────────────────
  // The routed id is what the server validates against, so it is also what the
  // picker lists from — `routedModelFor` builds exactly this string when the
  // tier is one of the agent's aliases.
  const routedModel = $derived(tier ? `${agentModel}-${tier}` : agentModel)
  const { efforts, default: agentEffort } = useModelEfforts(() => routedModel)
  // ── The seeded default: the AGENT-CONFIGURED effort for the routed model
  // (the pick saved beside the model in the agent editor) when there is one,
  // else the owner's platform default (Settings). Both are only ever offered
  // when this model publishes the level. `effortPristine` is what keeps an
  // explicit pick authoritative: choosing auto or a level is a decision about
  // THIS conversation, and re-applying a default after it would override the
  // one person whose opinion matters. It re-arms exactly when the pick stops
  // being valid — a tier switch to a model that cannot honor it — so the seed
  // follows the owner across models instead of stranding them on a level the
  // new model would 400 on.
  const prefs = useProfilePrefs()
  const preferredEffort = $derived(prefs.data?.preferredEffort ?? null)
  const seedEffort = $derived(agentEffort ?? preferredEffort)
  let effortPristine = $state(true)
  $effect(() => {
    if (effort && !efforts.includes(effort)) {
      effort = ''
      effortPristine = true
    }
    if (effortPristine) {
      const next = seedEffort && efforts.includes(seedEffort) ? seedEffort : ''
      if (effort !== next) effort = next
    }
  })

  // Load an existing conversation when the selection changes.
  $effect(() => {
    if (!conversationId || conversationId === convId) return
    abortCtrl?.abort()
    convId = conversationId
    // Same rule as the new-chat reset: switching threads drops any hold —
    // those messages were typed against a thread whose id never arrived, and
    // queueing them into the newly selected one would put them in a
    // conversation their author never saw.
    held = []
    userStopped = false
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

  // Having the thread open = having read it: advance the read cursor as
  // persisted turns land, so the rail's unread pill clears live — the same
  // contract the channel view runs. Synthetic streaming rows carry no seq,
  // so the cursor only moves for rows the server actually owns, and the
  // post-turn syncFromServer brings those in the moment a reply completes.
  // A read that covers the whole thread also clears the thread's bell rows
  // server-side; the refetch only fires when something was actually cleared
  // (the same invalidation set the bell's own mark-read runs).
  let lastReadPosted: { id: string; seq: number } = { id: '', seq: 0 }
  $effect(() => {
    const id = convId
    if (!id) return
    const latest = messages[messages.length - 1]?.seq ?? 0
    if (!latest) return
    const prev = lastReadPosted
    if (prev.id === id && latest <= prev.seq) return
    lastReadPosted = { id, seq: latest }
    void markConversationRead(id, latest)
      .then((r) => {
        void qc.invalidateQueries({ queryKey: ['conversations'] })
        if (r.cleared > 0) {
          void qc.invalidateQueries({ queryKey: ['notifications'] })
          void qc.invalidateQueries({ queryKey: ['home'] })
        }
      })
      .catch(() => {})
  })

  // Live-resume: poll the persisted state whenever the server owes us words we
  // didn't start streaming ourselves — a reload landed mid-generation (last
  // reply still 'streaming'), or the last message is the user's (a queued
  // message whose chained follow-up turn hasn't appeared yet — without this
  // the follow-up lands server-side but the chat never shows it). Capped so
  // it can't poll forever. A READER STOP suppresses it (see `userStopped`):
  // the poller's sync would hand back the server's still-streaming row and
  // the stopped reply would keep typing.
  const last = $derived(messages[messages.length - 1])
  const resuming = $derived(
    !streaming && !userStopped && (last?.role === 'user' || (last?.role === 'assistant' && last.status === 'streaming')),
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

  // The auto-resume watch — the visible half of the server's retry. A turn
  // whose STREAM died (not one the agent refused) is retried once, on the
  // same row, after a ~15s backoff: the server resurrects the errored row to
  // streaming and re-drives it. Without this watch the chat would show the
  // honest error line and then sit frozen while the retry streams server-side
  // — the exact "did it stall?" doubt this exists to kill. When the row flips
  // back to streaming, this watch stands down and the `resuming` poller above
  // animates the turn (AssistantTurn's resumed marker carries the story).
  let erroredAt = $state<number | null>(null)
  // Armed only when we WITNESS a turn die — our own reader below ending on
  // an error, or a row this view was animating flipping streaming→error. A
  // conversation opened straight onto an old error never polls. The
  // conversation id rides along so switching threads can't fake a transition.
  let prevStatusConv: string | null = ''
  let prevLastStatus: string | undefined
  $effect(() => {
    const s = last?.role === 'assistant' ? last.status : undefined
    if (convId === prevStatusConv && prevLastStatus === 'streaming' && s === 'error') {
      erroredAt = Date.now()
    }
    prevStatusConv = convId
    prevLastStatus = s
  })
  $effect(() => {
    if (erroredAt === null || streaming || userStopped) return
    // Waiting on the same dead turn, and only its FIRST death — a row already
    // stamped `resumed` that errored again is down for a person to look at.
    if (!(last?.role === 'assistant' && last.status === 'error' && last.resumed !== true)) return
    const id = convId
    if (!id) return
    let stop = false
    let ticks = 0
    const iv = setInterval(async () => {
      // ~90s: the backoff is 15s and the resurrect follows within moments;
      // past this window nothing is coming and the error line stands.
      if (stop || ++ticks > 110) return clearInterval(iv)
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

  // The surface's nudge — see the prop. Same last-signal guard the doc panes
  // use, so the effect only fires on a real bump.
  let lastSync = syncSignal
  $effect(() => {
    if (syncSignal === lastSync) return
    lastSync = syncSignal
    void syncFromServer()
  })

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
    // (Sending while the server owes a turn we didn't stream — the `resuming`
    // poller's watch — goes the fresh path below on purpose: the server demotes
    // it to a queue itself when a chained turn is already streaming, via the
    // `queued` event, and when one is not the fresh turn is exactly right.)
    if (streaming) {
      messages.push({ role: 'user', content: text, attachments: atts })
      if (convId) void enqueue(text, atts)
      // No conversation id yet — the first turn's response headers are the
      // only place it comes from. Hold locally; the flush happens the moment
      // the id lands (or the turn dies and the re-send path takes over).
      else held.push({ text, atts })
      return
    }

    await startTurn(text, atts, false)
  }

  /** Queue one message into the streaming turn. Never throws — a failed queue
   *  is a surfaced error, not a broken submit. */
  const enqueue = async (text: string, atts: Attachment[]) => {
    try {
      await queueChatMessage({
        model: agentModel,
        conversationId: convId!,
        content: text,
        tier: tier || undefined,
        effort: effort || undefined,
        ...splitAttachments(atts),
        kind,
      })
    } catch (e) {
      error = (e as Error).message
    }
  }

  /** Drain the hold. With a conversation id, everything queues server-side;
   *  without one AND nothing streaming, the first turn died before the server
   *  ever answered — the held messages never left, so the first of them starts
   *  a fresh turn (its message is already on screen, hence `shown`) and the
   *  rest follow it. Still streaming with no id: keep holding. */
  const flushHeld = async () => {
    while (held.length > 0) {
      if (convId) {
        const m = held.shift()!
        await enqueue(m.text, m.atts)
      } else if (!streaming) {
        const m = held.shift()!
        await startTurn(m.text, m.atts, true)
        // startTurn either produced a conversation id (loop continues; the
        // rest queue behind the turn that just ran) or failed the same way
        // the first turn did — loop retries with the next held message.
      } else return
    }
  }

  /** One streamed assistant turn. `shown` says the user's message is already
   *  in the transcript (the flush path put it there when it was sent). */
  const startTurn = async (text: string, atts: Attachment[], shown: boolean) => {
    if (!shown) messages.push({ role: 'user', content: text, attachments: atts })
    messages.push({ role: 'assistant', content: '', reasoning: '', tools: [], status: 'streaming' })
    streaming = true
    userStopped = false

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
            // The hold can drain the instant the id exists — the turn is
            // streaming, so everything lands in the server's queue.
            void flushHeld()
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
      if ((e as Error).name === 'AbortError') {
        // Stop means stop: freeze whatever arrived as this turn's final word
        // (see `userStopped`). An empty stop is an error bubble, same as an
        // empty stream — a silent frozen chat reads as broken.
        userStopped = true
        patchLast((m) => ({
          ...m,
          status: m.content || m.reasoning?.trim() || m.tools?.length ? 'complete' : 'error',
        }))
      } else error = (e as Error).message
    } finally {
      streaming = false
      abortCtrl = null
      // Pick up whatever happened meanwhile: queued messages, and the
      // follow-up turn the server chains for them (the resuming poller
      // animates it live once it appears). Skipped on a reader stop — a sync
      // here would hand the poller the server's still-streaming row and the
      // stopped reply would keep typing. Also the last chance to drain a
      // hold whose turn ended without ever producing an id.
      if (!userStopped) void syncFromServer()
      void flushHeld()
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

     The root PAINTS the ground. On the full-page stage that is a no-op (the
     page behind it is the same token), but everywhere the chat is EMBEDDED —
     a ticket's discussion tab, the plan split, a research run — the pane it
     fills sits on panel chrome (#141312 modal bodies, side-by-side splits).
     There, transparency read as a hole: the composer's opaque ground band
     floated on panel with no ground of its own, a dark patch with seams on
     three sides. `bg-surface` makes the container the chat's stage, and the
     transcript column and the composer float over it — one fill, edge to
     edge, no patches.

     `--chat-composer` is the measured height of the float, reserved at the
     bottom of the scroll so the last message never parks behind it. Measured,
     not a constant: the composer grows with the draft. -->
<div class="relative flex h-full w-full flex-col bg-surface" style:--chat-composer="{composerH}px">
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
            {kind === 'plan' ? `Plan with ${agentLabel}` : kind === 'ticket' ? 'Discuss the ticket' : `Talk to ${agentLabel}`}
          </div>
          <div class="font-sans text-sm text-muted">
            {kind === 'plan'
              ? 'Think through the work together, then draft tickets and send them to a board.'
              : kind === 'ticket'
                ? 'Everyone who can see this board is in the room — @mention to notify, attach files for the work.'
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
        placeholder={kind === 'ticket' ? 'Message the room — @ to mention' : `What would you like ${agentLabel} to work on?`}
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
        onStop={streaming ? stop : undefined}
      >
        {#snippet leftControls()}
          <!-- Attach stays live while a reply streams: a queued message may
               carry attachments (the server queue accepts them), same as the
               text. -->
          <AttachButton onAttach={(a) => attachments.push(a)} />
          {#if !minimal}
            <EmojiButton onPick={(ch) => composer?.insertText(ch)} />
            <!-- The handle lands in the editor; the value never does. See
                 RelayButton.svelte for why that is a property of the route it
                 took rather than a rule anybody has to remember. -->
            <RelayButton {agentModel} {agentLabel} onMinted={(h) => composer?.insertText(h)} disabled={streaming} />
          {/if}
        {/snippet}
        {#snippet rightControls()}
          <!-- Spec §7 rail order: tier chip, then effort — and the rail's last
               tile is send, which becomes stop while a reply streams
               (ChatComposer's onStop). The AGENT CHIP IS DELIBERATELY ABSENT:
               a conversation is bound to its agent and every host picks the
               agent in its own sidebar (comms rail, plan sidebar, research
               run) — a second switcher in the composer was a way to change
               the conversation's subject out from under the surface that owns
               it. MINIMAL mode (plan/research) drops the tier and effort chips
               too — the surface's chrome owns the model, and its contract is
               exactly attach, text, and submit. -->
          {#if !minimal}
            {#if tiers.length > 0}<TierPicker {tiers} value={tier} onChange={(t) => (tier = t)} />{/if}
            <!-- Effort sits immediately left of the send tile, and only when the
                 routed model's metadata vouches for levels — a model with no
                 published ladder shows no chip and its requests carry no effort.
                 Not disabled while streaming (TierPicker isn't either): a
                 queued message picks up the level set when it is sent. -->
            {#if efforts.length > 0}<EffortPicker {efforts} value={effort} onChange={(v) => { effort = v; effortPristine = false }} />{/if}
          {/if}
        {/snippet}
      </ChatComposer>
      </div>
    </div>
  </div>
  <ContextMenu {menu} />
</div>

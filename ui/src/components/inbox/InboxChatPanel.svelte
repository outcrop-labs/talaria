<script lang="ts">
  import { bottomStick } from '@/lib/stick-to-bottom'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Archive, Bot, ChevronDown, ChevronLeft, GripVertical, Paperclip, Plus, X } from '@lucide/svelte'
  import { relativeTime } from '@/lib/fleet'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import EffortPicker from '@/components/chat/EffortPicker.svelte'
  import PendingAttachments from '@/components/chat/PendingAttachments.svelte'
  import ChatComposer from '@/components/chat/ChatComposer.svelte'
  import type { ChatComposerHandle } from '@/components/chat/chat-composer'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'
  import { useProfilePrefs } from '@/lib/muse.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import StreamText from '@/components/chat/StreamText.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import CollapsePane from '@/components/ui/CollapsePane.svelte'
  import { fade, listStagger, slide, GROW_Y, QUICK } from '@/lib/motion'
  import { splitAttachments, uploadFile, type Attachment } from '@/lib/attachments'
  import { mergeInboxTimelinePages } from '@/lib/inbox-focus-timeline'
  import {
    DEFAULT_INBOX_PANEL_WIDTH,
    MAX_INBOX_PANEL_WIDTH,
    MIN_INBOX_PANEL_WIDTH,
    clampInboxPanelWidth,
    shouldCollapseInboxPanel,
  } from '@/lib/inbox-panel-size'
  import {
    archiveInboxConversation,
    createInboxConversation,
    useInboxConversations,
    useInboxFocusConversation,
    type FocusAssistant,
    type FocusItem,
    type InboxTimelineEntry,
  } from '@/lib/inbox-focus.svelte'
  import { useGoogleConnectStatus } from '@/lib/google-connect.svelte'
  import {
    PANEL_WIDTH_KEY,
    type PanelFocusContext,
    readPanelCollapsed,
    readPanelWidth,
    readSelectedChatId,
    subscribePanelCollapsed,
    subscribeSelectedChat,
    writePanelCollapsed,
    writePanelUnseen,
    writePanelWidth,
    writeSelectedChatId,
    type InboxCommandOptions,
    type StreamingTurn,
  } from './inbox-chat-panel'
  import TimelineEntry from './TimelineEntry.svelte'

  let {
    active,
    focusMode,
    surfaceLabel,
    assistant,
    busy,
    notice,
    streaming,
    onSubmit,
    onConfirm,
    onCancel,
    onRetry,
    onUndo,
  }: {
    /** The attached context: a queue FocusItem, or the lighter
     *  PanelFocusContext a surface like the brief hands over when the person
     *  asks about one of its lines. Only `key` and `question` are read — the
     *  row shows the question and the command carries the key. */
    active: FocusItem | PanelFocusContext | null
    focusMode: boolean
    /** The view the panel is floating over — shown under the assistant name so
     *  the conversation reads as being about where you are, not about Inbox. */
    surfaceLabel: string
    assistant: FocusAssistant | undefined
    busy: boolean
    notice: string | null
    streaming: StreamingTurn | null
    onSubmit: (instruction: string, options: InboxCommandOptions) => void
    onConfirm: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
    onCancel: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
    onRetry: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
    onUndo: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
  } = $props()

  // The owner named their assistant; use that name everywhere it appears. The
  // fallback is deliberately generic rather than a product name — an agent's
  // identity comes from its own persona, and hard-coding one here would put a
  // stranger's agent name in front of every customer.
  const assistantName = $derived(assistant?.name ?? 'your assistant')

  // Collapsed state persists in localStorage and stays in sync across this tab
  // (custom event) and other tabs (storage event) — the React version used
  // useSyncExternalStore; here a $state mirror updated by the subscription.
  let collapsed = $state(readPanelCollapsed())
  $effect(() => subscribePanelCollapsed(() => (collapsed = readPanelCollapsed())))
  const setCollapsed = (next: boolean) => writePanelCollapsed(next)

  // Saved width, likewise storage-synced. `committedWidth` mirrors the last
  // persisted value so drags that land on it can skip the write.
  let savedWidth = $state(DEFAULT_INBOX_PANEL_WIDTH)
  let committedWidth = DEFAULT_INBOX_PANEL_WIDTH
  $effect(() => {
    const syncWidth = () => {
      const next = readPanelWidth()
      committedWidth = next
      savedWidth = next
    }
    const syncStoredWidth = (event: StorageEvent) => {
      if (event.key === PANEL_WIDTH_KEY) syncWidth()
    }
    syncWidth()
    window.addEventListener('storage', syncStoredWidth)
    return () => window.removeEventListener('storage', syncStoredWidth)
  })
  function setSavedWidth(next: number) {
    const clamped = clampInboxPanelWidth(next)
    if (clamped === committedWidth) return
    committedWidth = clamped
    savedWidth = clamped
    writePanelWidth(clamped)
  }

  // ── Which conversation instance is on screen ─────────────────────────────
  //
  // SEGMENTATION IS THE CONTEXT STRATEGY: the picker at the top of the header
  // swaps instances, a fresh one sheds old context, and nothing imposes a
  // budget on a thread the person still wants. The selection persists like
  // collapse does (localStorage + event), so a reload reopens the thread being
  // read and a second tab follows a switch.
  const queryClient = useQueryClient()
  let chatId = $state<string | null>(readSelectedChatId())
  $effect(() => subscribeSelectedChat(() => (chatId = readSelectedChatId())))
  let menuOpen = $state(false)
  const chats = useInboxConversations()
  const chatList = $derived(chats.data?.conversations ?? [])
  const currentChat = $derived(chatList.find((c) => c.id === chatId) ?? null)
  // NOTE: deliberately NO "deselect when the id leaves the cached list"
  // watchdog. A selection missing from the list means THE LIST IS STALE (the
  // list refetches on its own), not that the selection is bad — the server
  // falls back to the most recent instance for any unknown id, so a stale
  // selection degrades to the right page anyway. A watchdog here fought the
  // adopter below in refetch races (adopt → list lands without it → deselect
  // → adopt → …) and the oscillation took the whole panel down with an
  // unhandled effect loop.
  const conversation = useInboxFocusConversation(chatId)
  // ADOPT, DON'T ASK. With nothing selected, the server answers from the most
  // recent instance (creating nothing) — and the picker should name whatever
  // is actually on screen, so the resolved id becomes the selection. This is
  // also how a first command on an empty account puts its new thread in the
  // picker without a round trip.
  //
  // GATED ON THE LIST, and that gate is load-bearing for the same race: an
  // instance the loaded list does not know (just-created, just-archived) is
  // never adopted, so this write cannot pick an id the next list refresh will
  // contradict — the loop the watchdog ran into cannot form.
  $effect(() => {
    const resolved = conversation.data?.pages[0]?.conversationId ?? null
    const known = !chats.isSuccess || chatList.some((c) => c.id === resolved)
    if (chatId === null && resolved && known) writeSelectedChatId(resolved)
  })

  async function startNewChat() {
    menuOpen = false
    try {
      const id = await createInboxConversation()
      writeSelectedChatId(id)
      await queryClient.invalidateQueries({ queryKey: ['inbox-conversations'] })
      window.setTimeout(() => composer?.focus(), 0)
    } catch {
      /* the list refreshes on its own cadence; a failed create is a failed click */
    }
  }

  async function archiveCurrentChat() {
    if (!chatId || busy) return
    menuOpen = false
    const id = chatId
    // ARCHIVE BEFORE DESELECTING. Clearing the selection first would start the
    // "most recent instance" read while the archive is still in flight — the
    // server would hand back the very chat being archived, the adopter would
    // re-select it, and the list refresh would drop it again. Delete, then
    // switch, then refresh: the fallback read never sees the dead chat.
    await archiveInboxConversation(id).catch(() => {})
    writeSelectedChatId(null)
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ['inbox-conversations'] }),
      queryClient.invalidateQueries({ queryKey: ['inbox-focus-conversation'] }),
    ])
  }
  let attachments = $state<Attachment[]>([])
  let attachmentError = $state<string | null>(null)
  // The effort pick for the next command ('' = the assistant model's default).
  // Offered only when the assistant's model publishes levels; the panel never
  // picks delegate/response models (attach + text + submit only), so the
  // assistant's own model is the one the pick is validated against.
  //
  // The seeded default is the assistant persona's CONFIGURED effort (the agent
  // editor's pick beside the assistant's model) when there is one, else the
  // owner's platform default (Settings) — and an explicit pick, including
  // auto, stays authoritative for the conversation. Same rule as ChatView.
  let effort = $state('')
  let effortPristine = $state(true)
  const { efforts, default: agentEffort } = useModelEfforts(() => assistant?.model ?? null)
  const prefs = useProfilePrefs()
  const preferredEffort = $derived(prefs.data?.preferredEffort ?? null)
  const seedEffort = $derived(agentEffort ?? preferredEffort)
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
  // Google connection state — the footer offers the connect link the moment
  // the assistant could use mail/calendar but can't (the chat reply only ever
  // says "not connected"; this is the way out of that sentence). Same cache
  // entry as every other Google status reader, so it costs one shared fetch.
  const google = useGoogleConnectStatus()
  let detachedKey = $state<string | null>(null)
  let dragWidth = $state<number | null>(null)
  let composer = $state<ChatComposerHandle | null>(null)
  let scroller = $state<HTMLDivElement | null>(null)
  let lastOutput: string | null = null
  let resize: {
    pointerId: number
    startX: number
    startWidth: number
    latestRawWidth: number
  } | null = null
  let resizeFrame: number | null = null
  let pendingDragWidth: number | null = null
  const attached = $derived(Boolean(active && detachedKey !== active.key))
  function toggleActiveDecisionAttachment() {
    if (!active) return
    detachedKey = detachedKey === active.key ? null : active.key
  }
  const panelWidth = $derived(dragWidth ?? savedWidth)
  const resizing = $derived(dragWidth !== null)

  const entries = $derived(mergeInboxTimelinePages((conversation.data?.pages ?? []).map((page) => page.entries)))

  function addAttachment(attachment: Attachment) {
    attachments = attachments.some((item) => item.id === attachment.id) ? attachments : [...attachments, attachment]
    attachmentError = null
  }

  async function uploadFiles(files: File[]) {
    attachmentError = null
    for (const file of files) {
      const result = await uploadFile(file)
      if ('error' in result) attachmentError = result.error
      else addAttachment(result)
    }
  }

  export function expand() {
    setCollapsed(false)
    writePanelUnseen(false)
    window.setTimeout(() => composer?.focus(), 0)
  }
  export function focus() {
    composer?.focus()
  }
  export function insertText(text: string) {
    composer?.insertText(text)
  }
  function collapse() {
    setCollapsed(true)
    // Focus follows the control that now owns opening this panel. It used to be
    // the chevron on the panel's own collapsed rail, which no longer exists —
    // and closing with the keyboard must not drop focus onto a removed node and
    // send the user back to the top of the document. The launcher advertises
    // itself with the attribute rather than being imported, so the panel keeps
    // knowing nothing about the nav sidebar.
    window.setTimeout(() => document.querySelector<HTMLElement>('[data-assistant-launcher]')?.focus(), 0)
  }

  function beginResize(event: PointerEvent) {
    if (event.button !== 0) return
    event.preventDefault()
    resize = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: panelWidth,
      latestRawWidth: panelWidth,
    }
    dragWidth = panelWidth
  }

  function finishResize(pointerId: number, cancelled = false) {
    const drag = resize
    if (!drag || drag.pointerId !== pointerId) return
    resize = null
    if (resizeFrame !== null) window.cancelAnimationFrame(resizeFrame)
    resizeFrame = null
    pendingDragWidth = null
    dragWidth = null
    if (cancelled) return
    if (shouldCollapseInboxPanel(drag.latestRawWidth)) collapse()
    else setSavedWidth(drag.latestRawWidth)
  }

  $effect(() => {
    if (!resizing) return
    const previousCursor = document.body.style.cursor
    const previousUserSelect = document.body.style.userSelect
    const moveResize = (event: PointerEvent) => {
      const drag = resize
      if (!drag || drag.pointerId !== event.pointerId) return
      const rawWidth = drag.startWidth + event.clientX - drag.startX
      drag.latestRawWidth = rawWidth
      pendingDragWidth = clampInboxPanelWidth(rawWidth)
      if (resizeFrame !== null) return
      resizeFrame = window.requestAnimationFrame(() => {
        resizeFrame = null
        const next = pendingDragWidth
        pendingDragWidth = null
        if (next !== null) dragWidth = next
      })
    }
    const completeResize = (event: PointerEvent) => finishResize(event.pointerId)
    const cancelPointerResize = (event: PointerEvent) => finishResize(event.pointerId, true)
    const cancelResize = () => {
      const pointerId = resize?.pointerId
      if (pointerId !== undefined) finishResize(pointerId, true)
    }

    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'
    window.addEventListener('pointermove', moveResize, true)
    window.addEventListener('pointerup', completeResize, true)
    window.addEventListener('pointercancel', cancelPointerResize, true)
    window.addEventListener('blur', cancelResize)
    return () => {
      document.body.style.cursor = previousCursor
      document.body.style.userSelect = previousUserSelect
      window.removeEventListener('pointermove', moveResize, true)
      window.removeEventListener('pointerup', completeResize, true)
      window.removeEventListener('pointercancel', cancelPointerResize, true)
      window.removeEventListener('blur', cancelResize)
    }
  })

  function resizeWithKeyboard(event: KeyboardEvent) {
    const step = event.shiftKey ? 64 : 24
    if (event.key === 'ArrowLeft') {
      event.preventDefault()
      if (panelWidth <= MIN_INBOX_PANEL_WIDTH) collapse()
      else setSavedWidth(panelWidth - step)
    } else if (event.key === 'ArrowRight') {
      event.preventDefault()
      setSavedWidth(panelWidth + step)
    } else if (event.key === 'Home') {
      event.preventDefault()
      collapse()
    } else if (event.key === 'End') {
      event.preventDefault()
      setSavedWidth(MAX_INBOX_PANEL_WIDTH)
    }
  }

  function onGlobalKeyDown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key === '\\') {
      event.preventDefault()
      if (collapsed) expand()
      else collapse()
    } else if (event.key === 'Escape' && !collapsed && window.innerWidth < 1400) {
      event.preventDefault()
      collapse()
    }
  }

  const latestOutput = $derived(
    [...entries].reverse().find((entry) => entry.kind === 'activity' || (entry.kind === 'message' && entry.role === 'assistant'))?.id ?? null,
  )
  $effect(() => {
    if (lastOutput && latestOutput && latestOutput !== lastOutput && collapsed) writePanelUnseen(true)
    lastOutput = latestOutput
  })

  // THE SHARED FOLLOWER, not a fifth copy. `lib/stick-to-bottom.ts` was
  // extracted from ChatView, ChannelView and ThreadPanel because all three had
  // written this by hand and all three shared one bug: they decided whether to
  // follow by measuring the scroll position AFTER the new content was in the
  // DOM, so any message taller than the threshold read as "the reader scrolled
  // up" and the transcript stopped following. It failed harder the longer the
  // message, which is exactly backwards.
  //
  // This panel had the fourth copy of it — `pinned`, set from an `onscroll`
  // handler, checked in an effect. The handler half was actually right (a scroll
  // event is the only honest signal that the reader moved), but the follow read
  // a `scrollHeight` that markdown had not finished rendering into, so a
  // streamed reply crept upward while the newest text sat below the fold.
  const stick = bottomStick()
  $effect(() => stick.attach(scroller))

  // OPEN AT THE NEWEST. The aside unmounts when the panel collapses, so every
  // expand (and every reload) starts at scrollTop 0 — the top of the newest
  // page, which is the OLDEST of what loaded. The newest turn is what the
  // owner opened the panel for. Runs exactly when the scroller is (re)bound,
  // and `jump` releases any hold left over from the last session so the
  // follow below can take over from the first delta.
  $effect(() => {
    if (!scroller) return
    stick.jump()
    stick.follow()
  })

  $effect(() => {
    // Deps: entry count and the streaming turn's text. The helper decides
    // whether to actually move — if the reader has scrolled away to read, it
    // does nothing.
    void entries.length
    void streaming?.content
    void streaming?.status
    stick.follow()
  })

  async function loadOlder() {
    const node = scroller
    const before = node?.scrollHeight ?? 0
    await conversation.fetchNextPage()
    requestAnimationFrame(() => {
      if (node) node.scrollTop += node.scrollHeight - before
    })
  }

  function submit(markdown: string) {
    const instruction = markdown.trim() || (attachments.length ? 'Review the attached context.' : '')
    if (!instruction || busy) return
    const split = splitAttachments(attachments)
    // This surface no longer offers delegate/model/mode picks (attach + text +
    // submit only), so the command always runs the assistant's own default in
    // the normal mode — the API fields stay, the choices just never come here.
    onSubmit(instruction, {
      conversationId: chatId,
      focusKey: attached && active ? active.key : null,
      delegateModel: null,
      responseModel: null,
      mode: 'normal',
      effort: effort || null,
      attachmentIds: split.attachmentIds,
      refs: split.refs,
    })
    // YOU ALWAYS SEE WHAT YOU JUST SAID (ChatView's rule). A reader parked up
    // in history stays parked through their own send otherwise — the hold
    // suppresses the follow, and the reply streams below the fold.
    stick.jump()
    composer?.clear()
    attachments = []
  }
</script>

<svelte:document onkeydown={onGlobalKeyDown} />

<!-- CollapsePane owns the collapse/expand width glide — same primitive as the
     nav rail. Desktop (≥1400px): the pane's width is the panel width and the
     aside fills it (clipping during the glide). Below 1400px the expanded
     panel is an OVERLAY: the pane contributes no flow width (w-0,
     overflow-visible) and the aside positions itself absolutely as before.

     COLLAPSED IS NOW ZERO WIDTH, NOT A RAIL. The panel used to keep a 44px
     strip parked beside the nav for its own expand chevron — a second vertical
     bar, present on every view, whose entire job was to reopen itself. The
     assistant is launched from the nav sidebar now (SidebarAssistant), so the
     strip is pure tax: it narrowed every page by a column and read as a piece
     of chrome nobody could name. Collapsed renders nothing and gives the width
     back; the glide is unchanged because CollapsePane only needs two fixed
     widths and 0 is one. -->
<CollapsePane
  collapsed={collapsed}
  collapsedWidth="w-0"
  width="w-0 min-[1400px]:w-[min(var(--panel-w),calc(100%_-_44px))]"
  animate={!resizing}
  style="--panel-w: {panelWidth}px"
  class="relative z-20 h-full shrink-0 overflow-visible min-[1400px]:overflow-hidden"
>
{#if !collapsed}
  <!-- fixed, not absolute: the pane (nearest positioned ancestor now) has zero
        width in overlay mode, so an absolute inset-0 backdrop would be zero-size. -->
  <button type="button" onclick={collapse} aria-label="Close assistant overlay" class="fixed inset-0 z-30 bg-black/45 min-[1400px]:hidden"></button>
  <!-- WIDTH IS MEASURED AGAINST THE VIEWPORT, NOT THE PANE. Overlay mode floats
        the aside inside a pane that is deliberately `w-0` (the note above says
        so, for the backdrop) — so the `calc(100% - 44px)` this used to carry
        computed to -44px, which is not a legal width, so the declaration was
        dropped and the aside shrink-to-fit against a zero-width container:
        a drawer squashed against the left edge. `100vw` is the box the "leave
        44px of the page showing" rule was always about. In flow mode (≥1400px)
        the pane already owns that arithmetic, so the aside simply fills it.

        THE HEIGHT CHAIN, which is the same methodology the NAV RAIL uses and
        this aside used to lack in flow mode. Below 1400px the aside is
        `absolute inset-y-0`, so its height is the pane's — definite, and the
        flex-1 transcript scrolls. At ≥1400px it turns `relative`, and a block
        child of the pane is AUTO height, not "fills it": with no definite
        height the flex-1 scroller grows to its content instead of scrolling,
        the panel becomes as tall as the whole conversation, and the composer
        sat below the app height, out of view. `min-[1400px]:h-full` is the
        NavRail's rule — the rail's inner column is `h-full` inside the
        `h-full` CollapsePane, and now the aside is too: header, scrolling
        transcript, composer, always exactly the pane's height. -->
  <aside
    class={cn(
      'absolute inset-y-0 left-0 z-40 flex w-[var(--aside-w)] shrink-0 flex-col border-r border-line bg-sidebar shadow-[var(--theme-shadow-3)] min-[1400px]:relative min-[1400px]:z-20 min-[1400px]:h-full min-[1400px]:w-full min-[1400px]:shadow-none',
    )}
    style:--aside-w="min({panelWidth}px, calc(100vw - 44px))"
    aria-label="Assistant conversation"
  >
    <div
      role="separator"
      aria-label="Resize assistant conversation"
      aria-orientation="vertical"
      aria-valuemin={MIN_INBOX_PANEL_WIDTH}
      aria-valuemax={MAX_INBOX_PANEL_WIDTH}
      aria-valuenow={Math.round(panelWidth)}
      tabindex="0"
      title="Drag to resize. Drag left past the minimum to collapse."
      ondblclick={() => setSavedWidth(DEFAULT_INBOX_PANEL_WIDTH)}
      onkeydown={resizeWithKeyboard}
      onpointerdown={beginResize}
      class="group absolute -right-2 inset-y-0 z-50 flex w-4 touch-none cursor-col-resize items-center justify-center focus-visible:!outline-none"
    >
      <span
        class={cn(
          'grid h-12 w-3 place-items-center rounded-full border border-line-strong bg-raised text-ink-dim opacity-0 shadow-[var(--theme-shadow-2)] transition-[opacity,color,background-color] group-hover:opacity-100 group-focus:opacity-100 group-focus:text-fg',
          resizing && 'bg-accent text-surface opacity-100',
        )}
        aria-hidden="true"
      >
        <GripVertical size={10} strokeWidth={1.75} />
      </span>
    </div>
    <header class="flex h-12 shrink-0 items-center gap-3 border-b border-line px-4">
      <span class="grid h-7 w-7 shrink-0 place-items-center rounded-md border border-line text-muted"><Bot size={14} /></span>
      <!-- THE AGENT, THEN THE THREAD. The primary line is WHO you are talking
           to; which conversation instance is the switchable thing, so it sits
           underneath as the picker. The old layout led with the thread and
           buried the assistant in the fine print. -->
      <div class="relative min-w-0 flex-1">
        <div class="truncate font-sans text-[13px] font-medium leading-tight text-fg">{assistantName}</div>
        <button
          type="button"
          onclick={() => (menuOpen = !menuOpen)}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          title="Switch conversation"
          class="-ml-1.5 mt-0.5 flex max-w-full items-center gap-1.5 rounded px-1.5 py-0.5 text-left transition-colors duration-[120ms] dither-fill hover:bg-raised"
        >
          <span class="truncate font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">
            {currentChat?.preview?.trim() || 'New conversation'}
          </span>
          <ChevronDown size={11} class="shrink-0 text-ink-dim" />
        </button>
        {#if menuOpen}
          <!-- fixed, not absolute: the menu has to close from a click anywhere,
               including outside the panel. -->
          <button type="button" tabindex="-1" aria-label="Close menu" onclick={() => (menuOpen = false)} class="fixed inset-0 z-40 cursor-default"></button>
          <div role="menu" class="absolute left-0 top-full z-50 mt-1 w-72 max-w-[calc(100vw-88px)] rounded-lg border border-line bg-sidebar p-1 shadow-[var(--theme-shadow-3)]">
            {#each chatList as c (c.id)}
              <button
                type="button"
                role="menuitem"
                onclick={() => {
                  writeSelectedChatId(c.id)
                  menuOpen = false
                }}
                class={cn('flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors duration-[120ms] dither-fill hover:bg-raised', c.id === chatId && 'bg-raised')}
              >
                <span class="min-w-0 flex-1">
                  <span class="block truncate font-sans text-[12px] text-fg">{c.preview?.trim() || 'New conversation'}</span>
                  <span class="block font-mono text-[8px] uppercase tracking-[0.06em] text-ink-dim">{relativeTime(c.updatedAt)}</span>
                </span>
              </button>
            {/each}
            {#if chatList.length > 0}<div class="my-1 h-px bg-line"></div>{/if}
            <button type="button" role="menuitem" onclick={() => void startNewChat()} class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 transition-colors duration-[120ms] dither-fill hover:bg-raised">
              <Plus size={12} class="text-ink-dim" />
              <span class="font-sans text-[12px] text-fg">New chat</span>
            </button>
            {#if currentChat}
              <button type="button" role="menuitem" onclick={() => void archiveCurrentChat()} disabled={busy} class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 transition-colors duration-[120ms] dither-fill hover:bg-raised disabled:opacity-50">
                <Archive size={12} class="text-ink-dim" />
                <span class="font-sans text-[12px] text-fg">Archive this chat</span>
              </button>
            {/if}
          </div>
        {/if}
      </div>
      <span class={cn('h-1.5 w-1.5 rounded-full', busy || conversation.data?.pages[0]?.working ? 'animate-pulse bg-success' : 'bg-line-strong')} aria-hidden="true"></span>
      <button type="button" onclick={collapse} aria-label="Collapse assistant conversation" aria-expanded={true} class="grid h-8 w-8 place-items-center rounded-md text-muted dither-fill hover:text-fg">
        <ChevronLeft size={14} />
      </button>
    </header>

    <div
      bind:this={scroller}
      class="min-h-0 flex-1 overflow-y-auto px-4 pb-6 pt-3"
      aria-live="polite"
    >
      {#if conversation.hasNextPage}
        <div class="mb-5 flex justify-center">
          <Button size="sm" variant="ghost" onclick={() => void loadOlder()} disabled={conversation.isFetchingNextPage}>
            {conversation.isFetchingNextPage ? 'Loading' : 'Load earlier history'}
          </Button>
        </div>
      {/if}
      {#if conversation.isLoading}
        <div class="space-y-4 py-4"><Skeleton class="h-16 w-4/5 rounded-lg" /><Skeleton class="ml-auto h-12 w-3/5 rounded-lg" /><Skeleton class="h-24 w-full rounded-lg" /></div>
      {:else if entries.length === 0 && !streaming}
        <div class="grid min-h-[320px] place-items-center text-center">
          <div class="max-w-xs">
            <span class="mx-auto grid h-10 w-10 place-items-center rounded-full border border-line text-muted"><Bot size={16} /></span>
            <h2 class="mt-4 font-sans text-base font-medium text-fg">{focusMode ? `Work through Inbox with ${assistantName}` : `Talk with ${assistantName} about ${surfaceLabel}`}</h2>
            <p class="mt-2 font-sans text-xs leading-5 text-muted">{focusMode ? 'The active decision is attached by default. Remove it to have a general, non-executing conversation.' : 'This conversation stays with you as you move through Talaria. Your assistant answers with its tools, reaching for the ones that match the view you are on first.'}</p>
          </div>
        </div>
      {:else}
        <div class="space-y-5" use:listStagger>
          {#each entries as entry (entry.id)}
            <div in:fade={{ duration: 150 }} out:fade={QUICK}>
              <TimelineEntry
                {entry}
                readOnly={!focusMode}
                {onConfirm}
                {onCancel}
                {onRetry}
                {onUndo}
              />
            </div>
          {/each}
          {#if streaming}
            <div class="ml-auto max-w-[86%] rounded-xl rounded-br-sm border border-line bg-raised px-3 py-2.5 font-sans text-[13px] leading-5 text-fg">
              <Markdown children={streaming.user} />
            </div>
            <div class="border-t border-line pt-4">
              <div class="mb-2 flex items-center gap-2 font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">
                <span class="h-1.5 w-1.5 animate-pulse rounded-full bg-success"></span>{streaming.status}
              </div>
              {#if streaming.content}<StreamText content={streaming.content} live class="font-sans text-[12.5px] leading-5 text-fg" />{/if}
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <div class="shrink-0 border-t border-line bg-sidebar/95 p-2 backdrop-blur">
      {#if notice}<div role="status" transition:slide={{ duration: 150 }} class="mb-2 rounded-md border border-line bg-panel px-3 py-2 font-sans text-[11px] leading-4 text-muted">{notice}</div>{/if}
      {#if attachmentError}<div role="alert" transition:slide={{ duration: 150 }} class="mb-2 rounded-md border border-danger/45 bg-panel px-3 py-2 font-sans text-[11px] leading-4 text-danger">{attachmentError}</div>{/if}
      {#if active}
        <!-- IN-FLOW row: slide={GROW_Y} on both legs so the composer glides as
             the row appears/leaves instead of snapping (ANIMATIONS.md). The
             panel's own collapse stays on its CSS width transition — this row
             only animates its own height. |global: in focus mode the panel
             mounts with a decision already active, so a local intro would be
             suppressed. -->
        <div transition:slide|global={GROW_Y} class="mb-2 flex min-w-0 items-center gap-2 rounded-md border border-line bg-surface px-2.5 py-2">
          <Paperclip size={12} class={attached ? 'text-accent' : 'text-ink-dim'} />
          <button type="button" onclick={toggleActiveDecisionAttachment} class="min-w-0 flex-1 truncate text-left font-sans text-[11px] text-muted">
            {attached ? active.question : 'Decision detached; general conversation'}
          </button>
          <button type="button" onclick={toggleActiveDecisionAttachment} aria-label={attached ? 'Detach active decision' : 'Attach active decision'} class="grid h-6 w-6 place-items-center rounded text-ink-dim dither-fill hover:text-fg">
            {#if attached}<X size={12} />{:else}<Paperclip size={12} />{/if}
          </button>
        </div>
      {/if}
      <div class="flex flex-col gap-2 rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]">
        <PendingAttachments items={attachments} onRemove={(id) => (attachments = attachments.filter((item) => item.id !== id))} />
        <ChatComposer
          bind:this={composer}
          placeholder={attached ? `Tell ${assistantName} what should happen…` : `Message ${assistantName}…`}
          onSubmit={submit}
          onFiles={(files) => void uploadFiles(files)}
          onEscape={() => window.innerWidth < 1400 && collapse()}
          disabled={busy}
          canSend={attachments.length > 0 || undefined}
        >
          {#snippet controlRail()}
            <!-- Attach left; spacer; effort + send right — the same rail
                 geometry every other composer uses (ChatView renders its
                 flex-1 spacer between leftControls and rightControls; a
                 controlRail host supplies its own). The send tile is pinned
                 to the rail's end by ChatComposer, immediately after the
                 effort chip. -->
            <AttachButton onAttach={addAttachment} disabled={busy} />
            <span class="flex-1"></span>
            {#if efforts.length > 0}<EffortPicker {efforts} value={effort} onChange={(v) => { effort = v; effortPristine = false }} disabled={busy} />{/if}
          {/snippet}
        </ChatComposer>
      </div>
      {#if google.data?.available && !google.data.connected}
        <!-- Where "I can't reach your mail" actually bites: the connect path,
             offered in the panel's own voice, one link, no lecture. The only
             thing that ever lived under the composer worth saying — the old
             status line beside it ("… orchestrates", "Tools on") said nothing
             the person could act on and is gone. -->
        <div class="mt-1.5 px-1 font-mono text-[8px] uppercase tracking-[0.06em] text-ink-dim">
          <a href="/api/integrations/google/connect" class="text-accent hover:underline">Connect Google →</a>
        </div>
      {/if}
    </div>
  </aside>
{/if}
</CollapsePane>

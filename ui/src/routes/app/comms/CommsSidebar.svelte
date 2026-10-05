<script lang="ts">
  import { untrack } from 'svelte'
  import { quintOut } from 'svelte/easing'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Archive, Ellipsis, MessagesSquare, Pencil, SendHorizontal, Settings, SquarePen, Star } from '@lucide/svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { prompt } from '@/components/ui/confirm.svelte'
  import { openCopyItems, type ContextMenuController, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import Rail from '@/components/app/Rail.svelte'
  import RailRow from '@/components/app/RailRow.svelte'
  import CountPill from '@/components/app/CountPill.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import AgentManageModal from '@/components/fleet/AgentManageModal.svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { delJson, patchJson } from '@/lib/fetch-json'
  import { rowCrossfade, slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import type { AgentModel } from '@/lib/agents'
  import type { AgentStatus } from '@/lib/fleet'
  import { useFleetDefs } from '@/lib/fleet-defs'
  import type { DirectoryUser } from '@/lib/users'
  import type { Channel } from '@/lib/channels.svelte'
  import { markConversationRead, type Conversation } from '@/lib/conversations.svelte'
  import type { CommsSelection } from '@/lib/comms-selection'
  import {
    agentPresence,
    buildSidebar,
    readCollapsed,
    writeCollapsed,
    type AgentRow,
    type Collapsed,
  } from '@/lib/comms-sidebar'
  import { agentStarKey, channelStarKey, threadStarKey } from '@/lib/comms-stars'
  import { useStars } from '@/lib/comms-stars.svelte'
  import { newAgentDraftKey } from '@/lib/comms-drafts'
  import { useDraftKeys } from '@/lib/comms-drafts.svelte'
  import { setTyping, typersIn } from '@/lib/comms-typing.svelte'
  import { groupDmCount, groupDmLabel, isGroupDm } from '@/lib/comms-dm'
  import { onUserEvent } from '@/lib/user-events.svelte'
  import TypingDots from '@/components/chat/TypingDots.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Section from './Section.svelte'
  import Hint from './Hint.svelte'
  import RailFailure from './RailFailure.svelte'
  import PresenceAvatar from './PresenceAvatar.svelte'
  import RowHoverCard from './RowHoverCard.svelte'
  import StatusBar from './StatusBar.svelte'

  // The Comms rail (KTD9), in this order (R14, R15):
  //
  //   Find a conversation
  //   Threads · Drafts & sent
  //   ───────
  //   ▾ Starred           your favorites, any kind (hidden when none)
  //   ▾ Channels          #channels
  //   ▾ Relays            ⇄ relays
  //   ▾ Direct messages   people
  //   ▾ Agents            each agent, its threads nested under it
  //
  // Comms keeps the selection and the routing; this owns which rows show
  // (lib/comms-sidebar.ts) and how they look. Every query arrives WHOLE, not
  // as `data`, because each section owns its failure marker — a friendly
  // "none yet" line over a 500 tells an owner to recreate what is still there.

  type Query<T> = {
    data: T | undefined
    isLoading: boolean
    isError: boolean
    error: unknown
    refetch: () => unknown
  }

  let {
    sel,
    selfId,
    channelsQuery,
    usersQuery,
    fleetQuery,
    conversationsQuery,
    fleetStatus,
    mayCreateChannels,
    mayStartRelays,
    menu,
    onSelect,
    onNewThread,
    onStartDm,
    onCreate,
    markRead,
  }: {
    sel: CommsSelection | null
    selfId: string | null
    channelsQuery: Query<Channel[]>
    usersQuery: Query<DirectoryUser[]>
    fleetQuery: Query<{ agents: AgentModel[] }>
    conversationsQuery: Query<Conversation[]>
    /** model → live fleet status, for the presence badge. */
    fleetStatus: Map<string, AgentStatus>
    mayCreateChannels: boolean
    mayStartRelays: boolean
    menu: ContextMenuController
    onSelect: (sel: CommsSelection) => void
    onNewThread: (model: string) => void
    onStartDm: (userId: string) => void
    onCreate: (name: string, kind: 'channel' | 'group') => void
    /** Advance a channel's read cursor to its latest message. */
    markRead: (channelId: string) => Promise<void>
  } = $props()

  const qc = useQueryClient()

  const channels = $derived(channelsQuery.data ?? [])
  const fleet = $derived(fleetQuery.data?.agents ?? [])
  const conversations = $derived(conversationsQuery.data ?? [])
  const users = $derived(usersQuery.data ?? [])
  const dmByPeer = $derived(new Map(channels.filter((c) => c.kind === 'dm').map((c) => [c.peer?.userId, c])))
  type Person = DirectoryUser & { dmId: string | null; unreadCount: number }
  const people: Person[] = $derived(
    users
      .filter((u) => u.id !== selfId)
      .map((u) => {
        const dm = dmByPeer.get(u.id)
        return { ...u, dmId: dm?.id ?? null, unreadCount: dm?.unreadCount ?? 0 }
      }),
  )

  // Group DMs carry their label into the model, so search and Starred see it.
  const agentLabel = (model: string) => fleet.find((a) => a.id === model)?.label ?? model
  type RailChannel = Channel & { group?: boolean; label?: string }
  const railChannels: RailChannel[] = $derived(
    channels.map((c) => (isGroupDm(c) ? { ...c, group: true, label: groupDmLabel(c, agentLabel) } : c)),
  )

  const activeChannelId = $derived(sel?.t === 'channel' ? sel.id : null)
  const agentSel = $derived(sel?.t === 'agent' ? sel : null)

  let query = $state('')
  // Read once at init; the toggle is the only writer afterwards.
  let collapsed = $state<Collapsed>(readCollapsed())
  const toggle = (section: keyof Collapsed) => {
    collapsed = { ...collapsed, [section]: !collapsed[section] }
    writeCollapsed(collapsed)
  }

  // Starred conversations — per person, shared with the header star buttons.
  const stars = useStars(() => selfId)
  // Conversations with a half-written message: a pencil on their row, and the
  // count on "Drafts & sent".
  const drafts = useDraftKeys(() => selfId)

  // Someone typing in one of your DMs, channels or relays: your own stream
  // says so (the server fans channel-level typing out to every other member),
  // and the dots play beside the row.
  $effect(() =>
    onUserEvent((e) => {
      if (e.type === 'typing') setTyping(e.channelId, e.userId, e.typing)
    }),
  )

  const view = $derived(
    buildSidebar(
      { channels: railChannels, people, agents: fleet, threads: conversations, stars: stars.keys },
      {
        query,
        collapsed,
        active: { channelId: activeChannelId, agentModel: agentSel?.model ?? null, threadId: agentSel?.conversationId ?? null },
      },
    ),
  )
  const roomCount = $derived(channels.filter((c) => c.kind === 'channel').length)
  const relayCount = $derived(channels.filter((c) => c.kind === 'group').length)

  // Agents whose threads are fanned out. Clicking an agent's row toggles it
  // (Codex's project rows); opening one of its threads by URL fans it out too.
  let expandedAgents = $state<Set<string>>(new Set())
  const toggleExpanded = (id: string) => {
    const next = new Set(expandedAgents)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    expandedAgents = next
  }
  $effect(() => {
    const model = agentSel?.conversationId ? agentSel.model : null
    if (model && !untrack(() => expandedAgents.has(model))) {
      expandedAgents = new Set([...untrack(() => expandedAgents), model])
    }
  })

  // An agent lists its newest threads first and stops at THREAD_CAP, with
  // "Show more" for the rest (Codex's project lists). Per agent, per visit.
  const THREAD_CAP = 5

  // Starring moves a row between its section and Starred; the pair shows the
  // move. Rows with no partner (search, collapse, fresh load) don't animate.
  const [sendRow, receiveRow] = rowCrossfade()

  // Archive slides the row away; anything else defers to the star crossfade.
  let archiving = $state<ReadonlySet<string>>(new Set())
  const rowOut = (node: Element, p: { key: string; id: string }) =>
    archiving.has(p.id) ? slide(node, { duration: 150, easing: quintOut }) : sendRow(node, { key: p.key })
  let showAllThreads = $state<Set<string>>(new Set())
  const toggleShowAll = (id: string) => {
    const next = new Set(showAllThreads)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    showAllThreads = next
  }

  // Which section's inline create input is open: Channels or Relays.
  let creating = $state<'channel' | 'group' | null>(null)
  const createInput = (kind: 'channel' | 'group', placeholder: string) =>
    creating === kind
      ? {
          placeholder,
          submit: (name: string | null) => {
            creating = null
            if (name) onCreate(name, kind)
          },
        }
      : null

  // ── row menus — shortcuts to what the rows already do ─────────────────────

  const starItem = (key: string): ContextMenuEntry => ({
    label: stars.has(key) ? 'Unstar' : 'Star',
    disabled: !selfId,
    onSelect: () => stars.toggle(key),
  })

  const channelRowMenu = (c: Channel): ContextMenuEntry[] => [
    ...openCopyItems(`/comms/channel/${c.id}`, () => onSelect({ t: 'channel', id: c.id })),
    { label: 'Mark read', disabled: !c.unreadCount, onSelect: () => void markRead(c.id) },
    starItem(channelStarKey(c.id)),
  ]

  // The agent's settings open in AgentManageModal, the /agents editor. Its
  // defs carry canManage (admins and the agent's managers); everyone else
  // sees the menu item disabled.
  const fleetDefs = useFleetDefs(() => true)
  const defFor = (model: string) => fleetDefs.data?.defs.find((d) => d.model === model) ?? null
  let editingModel = $state<string | null>(null)
  const editingDef = $derived(editingModel ? defFor(editingModel) : null)

  const agentRowMenu = (model: string): ContextMenuEntry[] => {
    const key = agentStarKey(model)
    const def = defFor(model)
    return [
      {
        label: stars.has(key) ? 'Unstar' : 'Star',
        icon: [Star, { size: 14 }],
        disabled: !selfId,
        onSelect: () => stars.toggle(key),
      },
      {
        label: def?.canManage === false ? 'Edit agent (managers only)' : 'Edit agent',
        icon: [Settings, { size: 14 }],
        disabled: !def?.canManage,
        onSelect: () => (editingModel = model),
      },
      'sep',
      { label: 'New thread', icon: [SquarePen, { size: 14 }], onSelect: () => onNewThread(model) },
    ]
  }

  const markThreadRead = async (id: string) => {
    try {
      // An absent seq tells the server "the thread's latest", so this needs no
      // load — and that whole-thread read clears the bell rows too.
      const r = await markConversationRead(id)
      void qc.invalidateQueries({ queryKey: ['conversations'] })
      if (r.cleared > 0) {
        void qc.invalidateQueries({ queryKey: ['notifications'] })
        void qc.invalidateQueries({ queryKey: ['home'] })
      }
    } catch (e) {
      toastError('Mark read failed', e)
    }
  }

  // Archiving takes a thread out of the rail (and out of Starred, if pinned).
  // An open thread hands its pane back to the agent's fresh one.
  const archiveThread = async (model: string, c: Conversation) => {
    archiving = new Set([...archiving, c.id])
    try {
      // DELETE archives (it hard-deletes only with ?hard=1); PATCH only restores.
      await delJson(`/api/conversations/${c.id}`)
    } catch (e) {
      archiving = new Set([...archiving].filter((id) => id !== c.id))
      toastError('Archive failed', e)
      return
    }
    // The refetch removes the row (its slide plays); only then un-star it, or
    // a starred thread flashes back under its agent for a frame.
    await qc.invalidateQueries({ queryKey: ['conversations'] })
    const key = threadStarKey(c.id)
    if (stars.has(key)) stars.toggle(key)
    if (agentSel?.model === model && agentSel.conversationId === c.id) onSelect({ t: 'agent', model, conversationId: null })
  }

  const threadRowMenu = (model: string, c: Conversation): ContextMenuEntry[] => [
    ...openCopyItems(`/comms/agent/${model}/${c.id}`, () => onSelect({ t: 'agent', model, conversationId: c.id })),
    ...(c.unreadCount ? [{ label: 'Mark read', onSelect: () => void markThreadRead(c.id) }] : []),
    {
      label: stars.has(threadStarKey(c.id)) ? 'Unstar' : 'Star',
      disabled: !selfId,
      onSelect: () => stars.toggle(threadStarKey(c.id)),
    },
    { label: 'Archive', onSelect: () => void archiveThread(model, c) },
    {
      label: 'Rename',
      onSelect: () => {
        void prompt({ title: 'Rename thread', defaultValue: c.title ?? '', placeholder: 'Thread name', confirmLabel: 'Rename' }).then(
          async (name) => {
            if (!name?.trim()) return
            try {
              await patchJson(`/api/conversations/${c.id}`, { title: name.trim() })
            } catch (e) {
              toastError('Rename failed', e)
            }
            void qc.invalidateQueries({ queryKey: ['conversations'] })
          },
        )
      },
    },
  ]

  // ── section header actions ────────────────────────────────────────────────

  const unreadRooms = $derived(channels.filter((c) => c.kind === 'channel' && (c.unreadCount ?? 0) > 0))
  const unreadRelays = $derived(channels.filter((c) => c.kind === 'group' && (c.unreadCount ?? 0) > 0))
  const unreadDms = $derived(channels.filter((c) => c.kind === 'dm' && (c.unreadCount ?? 0) > 0))
  const unreadThreads = $derived(conversations.filter((c) => (c.unreadCount ?? 0) > 0))

  // Starred's "Mark all read" covers every starred row, whatever its kind —
  // collapsed or not, so it reads the stars rather than the visible rows.
  const starredMore = (): ContextMenuEntry[] => {
    const all = buildSidebar(
      { channels: railChannels, people, agents: fleet, threads: conversations, stars: stars.keys },
      { query: '', collapsed: { ...collapsed, starred: false }, active: { channelId: null, agentModel: null } },
    ).starred
    const unreadChannels = all.flatMap((s) =>
      s.kind === 'channel' && (s.channel.unreadCount ?? 0) > 0
        ? [s.channel.id]
        : s.kind === 'person' && s.person.dmId && s.person.unreadCount > 0
          ? [s.person.dmId]
          : [],
    )
    const models = new Set(all.flatMap((s) => (s.kind === 'agent' ? [s.row.agent.id] : [])))
    const pinned = new Set(all.flatMap((s) => (s.kind === 'thread' ? [s.thread.id] : [])))
    const threads = unreadThreads.filter((c) => models.has(c.agentModel) || pinned.has(c.id))
    return [
      {
        label: 'Mark all read',
        disabled: unreadChannels.length === 0 && threads.length === 0,
        onSelect: () => {
          void Promise.all(unreadChannels.map((id) => markRead(id)))
          void Promise.all(threads.map((c) => markThreadRead(c.id)))
        },
      },
    ]
  }

  const channelsMore = (): ContextMenuEntry[] => [
    {
      label: 'Mark all read',
      disabled: unreadRooms.length === 0,
      onSelect: () => void Promise.all(unreadRooms.map((c) => markRead(c.id))),
    },
  ]

  const relaysMore = (): ContextMenuEntry[] => [
    {
      label: 'Mark all read',
      disabled: unreadRelays.length === 0,
      onSelect: () => void Promise.all(unreadRelays.map((c) => markRead(c.id))),
    },
  ]

  const dmsMore = (): ContextMenuEntry[] => [
    {
      label: 'Mark all read',
      disabled: unreadDms.length === 0,
      onSelect: () => void Promise.all(unreadDms.map((c) => markRead(c.id))),
    },
  ]
  const agentsMore = (): ContextMenuEntry[] => [
    {
      label: 'Mark all read',
      disabled: unreadThreads.length === 0,
      onSelect: () => void Promise.all(unreadThreads.map((c) => markThreadRead(c.id))),
    },
  ]


  // Trailing meta (time, pill, "new") gives way to the row's hover buttons.
  const hideOnHover = 'group-focus-within/row:invisible group-hover/row:invisible'
  const unreadName = (n: number | undefined) => ((n ?? 0) > 0 ? 'font-semibold text-fg' : '')

  // Silhouette widths for the rail-row skeletons — varied per index so the
  // sketch doesn't look stamped.
  const railW = ['w-24', 'w-32', 'w-20', 'w-28']

  const channelsFailed = $derived(channelsQuery.isError && channelsQuery.data === undefined)
  const usersFailed = $derived(usersQuery.isError && usersQuery.data === undefined)
  const fleetFailed = $derived(fleetQuery.isError && fleetQuery.data === undefined)
  const threadsFailed = $derived(conversationsQuery.isError && conversationsQuery.data === undefined)
  // While searching, a section with no matches hides — unless it failed,
  // because then "no match" is not something it knows.
  const showChannels = $derived(!view.searching || view.channels.length > 0 || channelsFailed)
  const showRelays = $derived(!view.searching || view.relays.length > 0 || channelsFailed)
  // Nothing starred (or every star's conversation gone): no section at all.
  const showStarred = $derived(view.searching ? view.starred.length > 0 : view.starredTotal > 0)
  const showDms = $derived(!view.searching || view.people.length > 0 || view.groupDms.length > 0 || usersFailed)
  const showAgents = $derived(!view.searching || view.agents.length > 0 || fleetFailed || threadsFailed)
</script>

<!-- One rail row's silhouette (RailRow anatomy: glyph lane + name, px-2
     py-1.5). `avatar` swaps the glyph square for the 5×5 avatar circle. -->
{#snippet railRowSkeleton(i: number, avatar: boolean)}
  <div aria-hidden="true" class="rounded-md px-2 py-1.5">
    <div class="flex h-5 items-center gap-1.5">
      <Skeleton class={avatar ? 'h-5 w-5 shrink-0 rounded-full' : 'h-3 w-3 shrink-0 rounded'} />
      <Skeleton class={`h-3 rounded-full ${railW[i % railW.length]}`} />
    </div>
  </div>
{/snippet}
{#snippet glyphRowSkeleton(i: number)}{@render railRowSkeleton(i, false)}{/snippet}
{#snippet avatarRowSkeleton(i: number)}{@render railRowSkeleton(i, true)}{/snippet}

{#snippet statusEmoji(u: DirectoryUser)}
  {#if u.statusEmoji}
    <span class="shrink-0 text-xs leading-none" title={u.statusText ?? undefined} aria-label={u.statusText ?? 'status'}>
      {u.statusEmoji}
    </span>
  {/if}
{/snippet}

{#snippet draftMark()}<Pencil size={12} class="shrink-0 text-muted" aria-label="Draft" />{/snippet}

<!-- A group DM: a count badge where an avatar would be, then everyone
     else's names (or the name it was given). -->
{#snippet groupDmRow(c: RailChannel)}
  <RailRow active={activeChannelId === c.id} onClick={() => onSelect({ t: 'channel', id: c.id })}>
    <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
    <span class="contents" oncontextmenu={(e) => menu.openMenu(e, channelRowMenu(c))}>
      <span
        aria-hidden="true"
        class="grid h-5 w-5 shrink-0 place-items-center rounded-[5px] bg-raised font-mono text-[10px] text-muted"
      >
        {groupDmCount(c)}
      </span>
      <span class={cn('min-w-0 truncate', unreadName(c.unreadCount))}>{c.label ?? c.name}</span>
      {#if typersIn(c.id, selfId).length > 0}
        <TypingDots class="mb-[5px] shrink-0 text-muted" label="Someone is typing" />
      {/if}
      <span class="flex-1"></span>
      {#if drafts.has(c.id)}{@render draftMark()}{/if}
      <CountPill count={c.unreadCount} />
    </span>
  </RailRow>
{/snippet}

<!-- The three row kinds, shared by their own sections and by Starred. -->
{#snippet channelRow(c: Channel)}
  <RailRow active={activeChannelId === c.id} onClick={() => onSelect({ t: 'channel', id: c.id })}>
    <!-- display:contents wrapper — carries the context menu without touching row layout -->
    <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
    <span class="contents" oncontextmenu={(e) => menu.openMenu(e, channelRowMenu(c))}>
      <!-- The Channel wire has no private flag yet, so every channel reads
           `#`; a relay reads `⇄`. -->
      <span class="w-3 shrink-0 text-center opacity-60">{c.kind === 'group' ? '⇄' : '#'}</span>
      <span class={cn('min-w-0 truncate', unreadName(c.unreadCount))}>{c.name}</span>
      {#if typersIn(c.id, selfId).length > 0}
        <TypingDots class="mb-[5px] shrink-0 text-muted" label="Someone is typing" />
      {/if}
      <span class="flex-1"></span>
      {#if drafts.has(c.id)}{@render draftMark()}{/if}
      <CountPill count={c.unreadCount} />
    </span>
  </RailRow>
{/snippet}

{#snippet personRow(u: Person)}
  <RailRow
    active={u.dmId !== null && activeChannelId === u.dmId}
    onClick={() => (u.dmId ? onSelect({ t: 'channel', id: u.dmId }) : onStartDm(u.id))}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
    <span
      class="contents"
      oncontextmenu={(e) => {
        const dm = u.dmId ? channels.find((c) => c.id === u.dmId) : undefined
        menu.openMenu(e, dm ? channelRowMenu(dm) : [{ label: 'Open', onSelect: () => onStartDm(u.id) }])
      }}
    >
      <PresenceAvatar name={u.name ?? u.email ?? '?'} src={u.picture} presence={u.online ? 'online' : 'offline'} />
      <span class={cn('min-w-0 truncate', unreadName(u.unreadCount))}>{u.name ?? u.email}</span>
      {@render statusEmoji(u)}
      {#if typersIn(u.dmId, selfId).includes(u.id)}
        <TypingDots class="mb-[5px] shrink-0 text-muted" label={`${u.name ?? u.email} is typing`} />
      {/if}
      <span class="flex-1"></span>
      {#if drafts.has(u.dmId)}{@render draftMark()}{/if}
      <CountPill count={u.unreadCount} />
    </span>
  </RailRow>
{/snippet}

<!-- Agent and thread rows are ONE hoverable piece (Codex's rows): the
     highlight spans the whole row, and its hover buttons sit inside it with
     no box of their own, in place of the trailing meta they hide. They are
     siblings of the row button, not children: a button cannot hold buttons. -->
{#snippet hoverActions(actions: import('svelte').Snippet)}
  <div class="absolute inset-y-0 right-1 hidden items-center gap-0.5 group-focus-within/row:flex group-hover/row:flex">
    {@render actions()}
  </div>
{/snippet}
{#snippet rowAction(title: string, onclick: (e: MouseEvent) => void, icon: import('svelte').Snippet, on?: boolean)}
  <button
    type="button"
    {title}
    aria-label={title}
    aria-pressed={on}
    class={cn(
      'grid h-6 w-6 place-items-center rounded text-muted transition-[color,scale] duration-150 ease-[cubic-bezier(0.25,0.46,0.45,0.94)] motion-safe:active:scale-[0.97] hover:text-fg focus-visible:text-fg focus-visible:outline-none',
      on && 'text-accent hover:text-accent',
    )}
    onclick={(e) => {
      e.stopPropagation()
      onclick(e)
    }}
  >
    {@render icon()}
  </button>
{/snippet}

{#snippet threadCard(a: AgentModel, c: Conversation)}
  <div class="flex items-baseline gap-2">
    <span class="min-w-0 flex-1 truncate text-[13px] font-medium text-fg">{c.title || 'Untitled'}</span>
    <span class="shrink-0 font-mono text-[10px] text-ink-dim">{relativeTime(c.updatedAt)}</span>
  </div>
  <div class="mt-1 flex items-center gap-1.5">
    <PresenceAvatar name={a.label} presence={agentPresence(fleetStatus.get(a.id))} />
    <span class="truncate">{a.label}</span>
    {#if c.working}<span class="text-success">· working</span>{:else if c.failed}<span class="text-danger">· needs a re-run</span>{/if}
    {#if c.unreadCount}<span>· {c.unreadCount} unread</span>{/if}
  </div>
{/snippet}

<!-- One thread with an agent: nested under its agent, or on its own in
     Starred once starred (then it leads with the agent's avatar). -->
{#snippet threadRow(a: AgentModel, c: Conversation, nested: boolean)}
  {@const activeThread = agentSel?.model === a.id && agentSel?.conversationId === c.id}
  {@const starKey = threadStarKey(c.id)}
  {@const starred = stars.has(starKey)}
  <RowHoverCard>
    {#snippet card()}{@render threadCard(a, c)}{/snippet}
    <div class="group/row relative rounded-md hover:bg-card2">
      <RailRow
        active={activeThread}
        onClick={() => onSelect({ t: 'agent', model: a.id, conversationId: c.id })}
        class={cn(nested && 'pl-7', unreadName(c.unreadCount))}
      >
        <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
        <span class="contents" oncontextmenu={(e) => menu.openMenu(e, threadRowMenu(a.id, c))}>
          {#if nested}
            <!-- §10 session-row anatomy, shared with the Plan rail. -->
            <SessionRowBody
              conv={c}
              active={activeThread}
              metaClass={hideOnHover}
              showUnread={false}
              draft={drafts.has(c.id)}
              waitingSite="comms/thread-working"
            />
          {:else}
            <PresenceAvatar name={a.label} presence={agentPresence(fleetStatus.get(a.id))} />
            <span class="min-w-0 flex-1 truncate">{c.title || 'Untitled'}</span>
            {#if drafts.has(c.id)}<span class={cn('shrink-0', hideOnHover)}>{@render draftMark()}</span>{/if}
            {#if c.working && !c.failed}
              <span class={cn('shrink-0', hideOnHover)}><WaitingMark site="comms/thread-working" size={12} class="text-accent" /></span>
            {/if}
          {/if}
        </span>
      </RailRow>
      {#snippet starIcon()}<Star size={13} fill={starred ? 'currentColor' : 'none'} />{/snippet}
      {#snippet archiveIcon()}<Archive size={13} />{/snippet}
      {#snippet actions()}
        {@render rowAction(starred ? 'Unstar' : 'Star', () => selfId && stars.toggle(starKey), starIcon, starred)}
        {@render rowAction('Archive thread', () => void archiveThread(a.id, c), archiveIcon)}
      {/snippet}
      {@render hoverActions(actions)}
    </div>
  </RowHoverCard>
{/snippet}

{#snippet agentRow(row: AgentRow<AgentModel, Conversation>)}
  {@const a = row.agent}
  {@const freshThread = agentSel?.model === a.id && agentSel?.conversationId === null}
  <!-- Starred threads list in Starred, not here. -->
  {@const agentThreads = conversations.filter((c) => c.agentModel === a.id && !view.starredThreadIds.has(c.id))}
  <!-- Clicking the row fans its threads out or folds them away. While
       searching, the matching threads show, uncapped. -->
  {@const expanded = row.matchedThreads ? row.matchedThreads.length > 0 : expandedAgents.has(a.id)}
  {@const showAll = showAllThreads.has(a.id)}
  {@const threads = row.matchedThreads ?? (expanded ? (showAll ? agentThreads : agentThreads.slice(0, THREAD_CAP)) : [])}
  {@const role = a.role ? a.role.charAt(0).toUpperCase() + a.role.slice(1) : null}
  {@const presence = agentPresence(fleetStatus.get(a.id))}
  <div class="space-y-0.5">
    <RowHoverCard>
      {#snippet card()}
        <div class="flex items-center gap-2">
          <PresenceAvatar name={a.label} {presence} />
          <span class="min-w-0 flex-1 truncate text-[13px] font-medium text-fg">{a.label}</span>
          <span class={cn('shrink-0 text-[11px]', presence === 'online' ? 'text-success' : 'text-ink-dim')}>
            {presence === 'online' ? 'Active' : 'Offline'}
          </span>
        </div>
        {#if role}<div class="mt-1">{role}</div>{/if}
        <div class="mt-1 text-ink-dim">
          {agentThreads.length === 0 ? 'No threads yet' : `${agentThreads.length} ${agentThreads.length === 1 ? 'thread' : 'threads'}`}{#if agentThreads.some((c) => c.working)}<span class="text-success"> · working</span>{/if}
        </div>
      {/snippet}
      <div class="group/row relative rounded-md hover:bg-card2">
        <RailRow active={freshThread} onClick={() => toggleExpanded(a.id)}>
          <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
          <span
            class="contents"
            aria-expanded={agentThreads.length > 0 ? expanded : undefined}
            oncontextmenu={(e) => menu.openMenu(e, agentRowMenu(a.id))}
          >
            <!-- R19: agents have no photo on the wire yet — initials. -->
            <PresenceAvatar name={a.label} {presence} />
            <span class={cn('min-w-0 flex-1 truncate', unreadName(row.unread))}>{a.label}</span>
            <span class={cn('flex shrink-0 items-center gap-1.5', hideOnHover)}>
              <!-- A draft for a new thread, or — folded — in any of its threads. -->
              {#if drafts.has(newAgentDraftKey(a.id)) || (!expanded && agentThreads.some((c) => drafts.has(c.id)))}
                {@render draftMark()}
              {/if}
              {#if agentThreads.some((c) => c.working && !c.failed)}
                <span class="contents" title="Working on a reply">
                  <WaitingMark site="comms/agent-working" size={12} class="shrink-0 text-accent" />
                </span>
              {/if}
              {#if freshThread}
                <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">new</span>
              {/if}
            </span>
          </span>
        </RailRow>
        {#snippet moreIcon()}<Ellipsis size={14} />{/snippet}
        {#snippet newIcon()}<SquarePen size={13} />{/snippet}
        {#snippet actions()}
          {@render rowAction('More', (e) => menu.openMenu(e, agentRowMenu(a.id)), moreIcon)}
          {@render rowAction(`New thread with ${a.label}`, () => onNewThread(a.id), newIcon)}
        {/snippet}
        {@render hoverActions(actions)}
      </div>
    </RowHoverCard>
    {#each threads as c (c.id)}
      <div in:receiveRow={{ key: threadStarKey(c.id) }} out:rowOut={{ key: threadStarKey(c.id), id: c.id }}>
        {@render threadRow(a, c, true)}
      </div>
    {/each}
    {#if !row.matchedThreads && expanded && agentThreads.length > THREAD_CAP}
      <button
        type="button"
        class="w-full rounded-md px-2 py-1.5 pl-7 text-left font-sans text-[13px] text-ink-dim transition-colors hover:bg-card2 hover:text-fg"
        onclick={() => toggleShowAll(a.id)}
      >
        {showAll ? 'Show less' : 'Show more'}
      </button>
    {/if}
  </div>
{/snippet}

<Rail>
  {#snippet footer()}<StatusBar />{/snippet}

  <!-- R14: search filters every section below it. -->
  <div class="mb-2 px-1">
    <Input
      size="sm"
      type="search"
      bind:value={query}
      placeholder="Find a conversation"
      aria-label="Find a conversation"
      onkeydown={(e) => {
        if (e.key === 'Escape') query = ''
      }}
    />
  </div>

  <div class="space-y-0.5">
    <RailRow active={sel?.t === 'threads'} onClick={() => onSelect({ t: 'threads' })}>
      <MessagesSquare size={14} class="shrink-0 opacity-60" />
      <span class="min-w-0 flex-1 truncate">Threads</span>
    </RailRow>
    <RailRow active={sel?.t === 'drafts'} onClick={() => onSelect({ t: 'drafts' })}>
      <SendHorizontal size={14} class="shrink-0 opacity-60" />
      <span class="min-w-0 flex-1 truncate">Drafts & sent</span>
      {#if drafts.keys.size > 0}
        <span
          class="flex shrink-0 items-center gap-1 font-mono text-[11px] text-muted"
          title={`${drafts.keys.size} unsent ${drafts.keys.size === 1 ? 'draft' : 'drafts'}`}
        >
          <Pencil size={11} />{drafts.keys.size}
        </span>
      {/if}
    </RailRow>
  </div>
  <div role="separator" class="mx-2 my-3 border-t border-line-subtle"></div>

  {#if view.noMatches && !channelsFailed && !usersFailed && !fleetFailed}
    <EmptyState variant="inline" class="px-2 py-1" title="No conversations match" />
  {/if}

  {#if showStarred}
    <Section label="Starred" collapsed={collapsed.starred} onToggle={() => toggle('starred')} moreItems={starredMore}>
      {#each view.starred as s (s.key)}
        <div in:receiveRow={{ key: s.key }} out:rowOut={{ key: s.key, id: s.kind === 'thread' ? s.thread.id : '' }}>
          {#if s.kind === 'channel' && s.channel.group}
            {@render groupDmRow(s.channel)}
          {:else if s.kind === 'channel'}
            {@render channelRow(s.channel)}
          {:else if s.kind === 'person'}
            {@render personRow(s.person)}
          {:else if s.kind === 'thread'}
            {@render threadRow(s.agent, s.thread, false)}
          {:else}
            {@render agentRow(s.row)}
          {/if}
        </div>
      {/each}
    </Section>
  {/if}

  {#if showChannels}
    <Section
      label="Channels"
      collapsed={collapsed.channels}
      onToggle={() => toggle('channels')}
      addTitle="New channel"
      onAdd={mayCreateChannels ? () => (creating = 'channel') : undefined}
      moreItems={channelsMore}
      create={createInput('channel', 'channel name')}
      loading={channelsQuery.isLoading}
      count={4}
      rowSkeleton={glyphRowSkeleton}
    >
      {#each view.channels as c (c.id)}
        <div in:receiveRow={{ key: channelStarKey(c.id) }} out:sendRow={{ key: channelStarKey(c.id) }}>
          {@render channelRow(c)}
        </div>
      {/each}
      {#if roomCount === 0 && !view.searching}
        {#if channelsFailed}
          <RailFailure
            error={channelsQuery.error}
            title="Could not load channels"
            onRetry={() => void channelsQuery.refetch()}
          />
        {:else if !channelsQuery.isLoading}
          <Hint>Ambient, persistent talk.</Hint>
        {/if}
      {/if}
    </Section>
  {/if}

  {#if showRelays}
    <Section
      label="Relays"
      collapsed={collapsed.relays}
      onToggle={() => toggle('relays')}
      addTitle="New relay"
      onAdd={mayStartRelays ? () => (creating = 'group') : undefined}
      moreItems={relaysMore}
      create={createInput('group', "what's it about?")}
      loading={channelsQuery.isLoading}
      count={2}
      rowSkeleton={glyphRowSkeleton}
    >
      {#each view.relays as c (c.id)}
        <div in:receiveRow={{ key: channelStarKey(c.id) }} out:sendRow={{ key: channelStarKey(c.id) }}>
          {@render channelRow(c)}
        </div>
      {/each}
      {#if relayCount === 0 && !view.searching && !channelsFailed && !channelsQuery.isLoading}
        <Hint>Gather people + agents around a purpose; conclude when done.</Hint>
      {/if}
    </Section>
  {/if}

  {#if showDms}
    <Section
      label="Direct messages"
      collapsed={collapsed.dms}
      onToggle={() => toggle('dms')}
      addTitle="New message"
      onAdd={() => onSelect({ t: 'new' })}
      moreItems={dmsMore}
      loading={usersQuery.isLoading}
      count={3}
      rowSkeleton={avatarRowSkeleton}
    >
      {#each view.people as u (u.id)}
        {@const key = u.dmId ? channelStarKey(u.dmId) : `person:${u.id}`}
        <div in:receiveRow={{ key }} out:sendRow={{ key }}>
          {@render personRow(u)}
        </div>
      {/each}
      {#each view.groupDms as c (c.id)}
        <div in:receiveRow={{ key: channelStarKey(c.id) }} out:sendRow={{ key: channelStarKey(c.id) }}>
          {@render groupDmRow(c)}
        </div>
      {/each}
      {#if people.length === 0 && !view.searching}
        {#if usersFailed}
          <!-- "Just you so far." over a failed directory read is how a
               20-person org gets told it is one person. -->
          <RailFailure error={usersQuery.error} title="Could not load teammates" onRetry={() => void usersQuery.refetch()} />
        {:else if !usersQuery.isLoading && !collapsed.dms}
          <Hint>Just you so far.</Hint>
        {/if}
      {/if}
    </Section>
  {/if}

  {#if showAgents}
    <Section
      label="Agents"
      collapsed={collapsed.agents}
      onToggle={() => toggle('agents')}
      moreItems={agentsMore}
      loading={fleetQuery.isLoading}
      count={3}
      rowSkeleton={avatarRowSkeleton}
    >
      {#each view.agents as row (row.agent.id)}
        <div in:receiveRow={{ key: agentStarKey(row.agent.id) }} out:sendRow={{ key: agentStarKey(row.agent.id) }}>
          {@render agentRow(row)}
        </div>
      {/each}
      {#if fleet.length === 0 && !view.searching}
        {#if fleetFailed}
          <RailFailure error={fleetQuery.error} title="Could not load your agents" onRetry={() => void fleetQuery.refetch()} />
        {:else if !fleetQuery.isLoading && !collapsed.agents}
          <Hint>No agents yet. Hire on /agents.</Hint>
        {/if}
      {/if}
      <!-- Threads nest under the agent rows above, so a failed conversation
           read makes every agent look like it has never been talked to.
           The rows stay (good fleet data is not thrown away) — the marker
           says the threads are missing rather than absent. -->
      {#if threadsFailed}
        <div transition:slide={{ duration: 150 }}>
          <RailFailure
            error={conversationsQuery.error}
            title="Could not load your threads"
            onRetry={() => void conversationsQuery.refetch()}
          />
        </div>
      {/if}
    </Section>
  {/if}
</Rail>

{#if editingDef && fleetDefs.data}
  <AgentManageModal
    open
    onClose={() => (editingModel = null)}
    def={editingDef}
    endpoints={fleetDefs.data.endpoints}
    canManage={editingDef.canManage}
  />
{/if}

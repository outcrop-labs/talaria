<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { MessagesSquare, SendHorizontal } from '@lucide/svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Input from '@/components/ui/Input.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { prompt } from '@/components/ui/confirm.svelte'
  import { openCopyItems, type ContextMenuController, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import Rail from '@/components/app/Rail.svelte'
  import RailRow from '@/components/app/RailRow.svelte'
  import CountPill from '@/components/app/CountPill.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import { cn } from '@/lib/cn'
  import { patchJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import type { AgentModel } from '@/lib/agents'
  import type { AgentStatus } from '@/lib/fleet'
  import type { DirectoryUser } from '@/lib/users'
  import type { Channel } from '@/lib/channels.svelte'
  import { markConversationRead, type Conversation } from '@/lib/conversations.svelte'
  import type { CommsSelection } from '@/lib/comms-selection'
  import { agentPresence, buildSidebar, readCollapsed, writeCollapsed, type Collapsed } from '@/lib/comms-sidebar'
  import Section from './Section.svelte'
  import Hint from './Hint.svelte'
  import RailFailure from './RailFailure.svelte'
  import PresenceAvatar from './PresenceAvatar.svelte'

  // The Comms rail (KTD9), Slack-ordered (R14, R15):
  //
  //   Find a conversation
  //   Threads · Drafts & sent
  //   ───────
  //   ▾ Channels          #channels, then ⇄ relays
  //   ▾ Direct messages   people, then agents (threads nested)
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
    onOpenAgent,
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
    /** The agent's working thread if one is live, else a fresh one. */
    onOpenAgent: (model: string) => void
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
  const people = $derived(
    users
      .filter((u) => u.id !== selfId)
      .map((u) => {
        const dm = dmByPeer.get(u.id)
        return { ...u, dmId: dm?.id ?? null, unreadCount: dm?.unreadCount ?? 0 }
      }),
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

  const view = $derived(
    buildSidebar(
      { channels, people, agents: fleet, threads: conversations },
      { query, collapsed, active: { channelId: activeChannelId, agentModel: agentSel?.model ?? null } },
    ),
  )
  const roomCount = $derived(channels.filter((c) => c.kind !== 'dm').length)

  // Agents whose thread list is pinned open (chevron) without being selected.
  let expandedAgents = $state<Set<string>>(new Set())
  const toggleExpanded = (id: string) => {
    const next = new Set(expandedAgents)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    expandedAgents = next
  }

  // Which kind of thing the Channels section's inline input is creating.
  let creating = $state<'channel' | 'group' | null>(null)

  // ── row menus — shortcuts to what the rows already do ─────────────────────

  const channelRowMenu = (c: Channel): ContextMenuEntry[] => [
    ...openCopyItems(`/comms/channel/${c.id}`, () => onSelect({ t: 'channel', id: c.id })),
    { label: 'Mark read', disabled: !c.unreadCount, onSelect: () => void markRead(c.id) },
  ]

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

  const threadRowMenu = (model: string, c: Conversation): ContextMenuEntry[] => [
    ...openCopyItems(`/comms/agent/${model}/${c.id}`, () => onSelect({ t: 'agent', model, conversationId: c.id })),
    ...(c.unreadCount ? [{ label: 'Mark read', onSelect: () => void markThreadRead(c.id) }] : []),
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

  const unreadRooms = $derived(channels.filter((c) => c.kind !== 'dm' && (c.unreadCount ?? 0) > 0))
  const unreadDms = $derived(channels.filter((c) => c.kind === 'dm' && (c.unreadCount ?? 0) > 0))
  const unreadThreads = $derived(conversations.filter((c) => (c.unreadCount ?? 0) > 0))

  const channelsMore = (): ContextMenuEntry[] => [
    { label: 'New relay', disabled: !mayStartRelays, onSelect: () => (creating = 'group') },
    'sep',
    {
      label: 'Mark all read',
      disabled: unreadRooms.length === 0,
      onSelect: () => void Promise.all(unreadRooms.map((c) => markRead(c.id))),
    },
  ]

  const dmsMore = (): ContextMenuEntry[] => [
    {
      label: 'Mark all read',
      disabled: unreadDms.length === 0 && unreadThreads.length === 0,
      onSelect: () => {
        void Promise.all(unreadDms.map((c) => markRead(c.id)))
        void Promise.all(unreadThreads.map((c) => markThreadRead(c.id)))
      },
    },
  ]

  // The people picker behind Direct messages' `+`: the directory, as menu
  // rows with their avatars. Picking someone opens (or creates) the DM.
  const dmPicker = (): ContextMenuEntry[] =>
    people.length === 0
      ? [{ label: 'No teammates yet', disabled: true }]
      : people.map((u) => ({
          label: u.name ?? u.email ?? 'teammate',
          icon: [Avatar, { name: u.name ?? u.email ?? '?', src: u.picture ?? null, class: 'h-5 w-5 text-[9px]' }],
          onSelect: () => (u.dmId ? onSelect({ t: 'channel', id: u.dmId }) : onStartDm(u.id)),
        }))

  const meta = (n: number) => (n > 0 ? String(n).padStart(2, '0') : undefined)
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
  const showDms = $derived(
    !view.searching || view.people.length > 0 || view.agents.length > 0 || usersFailed || fleetFailed,
  )
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

<Rail>
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
    </RailRow>
  </div>
  <div role="separator" class="mx-2 my-3 border-t border-line-subtle"></div>

  {#if view.noMatches && !channelsFailed && !usersFailed && !fleetFailed}
    <EmptyState variant="inline" class="px-2 py-1" title="No conversations match" />
  {/if}

  {#if showChannels}
    <Section
      label="Channels"
      meta={meta(roomCount)}
      collapsed={collapsed.channels}
      onToggle={() => toggle('channels')}
      addTitle="New channel"
      onAdd={mayCreateChannels ? () => (creating = 'channel') : undefined}
      moreItems={channelsMore}
      create={creating
        ? {
            placeholder: creating === 'channel' ? 'channel name' : "relay: what's it about?",
            submit: (name) => {
              const kind = creating
              creating = null
              if (name && kind) onCreate(name, kind)
            },
          }
        : null}
      loading={channelsQuery.isLoading}
      count={4}
      rowSkeleton={glyphRowSkeleton}
    >
      {#each view.channels as c (c.id)}
        <RailRow active={activeChannelId === c.id} onClick={() => onSelect({ t: 'channel', id: c.id })}>
          <!-- display:contents wrapper — carries the context menu without touching row layout -->
          <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
          <span class="contents" oncontextmenu={(e) => menu.openMenu(e, channelRowMenu(c))}>
            <!-- The Channel wire has no private flag yet, so every channel
                 reads `#`; relays keep ⇄. -->
            <span class="w-3 shrink-0 text-center opacity-60">{c.kind === 'group' ? '⇄' : '#'}</span>
            <span class={cn('min-w-0 flex-1 truncate', unreadName(c.unreadCount))}>{c.name}</span>
            <CountPill count={c.unreadCount} />
          </span>
        </RailRow>
      {/each}
      {#if roomCount === 0 && !view.searching}
        {#if channelsFailed}
          <RailFailure
            error={channelsQuery.error}
            title="Could not load channels"
            onRetry={() => void channelsQuery.refetch()}
          />
        {:else if !channelsQuery.isLoading}
          <Hint>Channels for ambient talk; relays gather people + agents around a purpose.</Hint>
        {/if}
      {/if}
    </Section>
  {/if}

  {#if showDms}
    <Section
      label="Direct messages"
      meta={meta(people.length + fleet.length)}
      collapsed={collapsed.dms}
      onToggle={() => toggle('dms')}
      addTitle="Start a direct message"
      addItems={dmPicker}
      moreItems={dmsMore}
      loading={usersQuery.isLoading || fleetQuery.isLoading}
      count={4}
      rowSkeleton={avatarRowSkeleton}
    >
      {#each view.people as u (u.id)}
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
            <span class="flex-1"></span>
            <CountPill count={u.unreadCount} />
          </span>
        </RailRow>
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

      {#each view.agents as row (row.agent.id)}
        {@const a = row.agent}
        {@const activeAgent = agentSel?.model === a.id}
        {@const agentThreads = conversations.filter((c) => c.agentModel === a.id)}
        <!-- Threads unfold for the active agent, or via the chevron —
             peeking at an agent's threads shouldn't require selecting it.
             While searching, the matching threads show, uncapped. -->
        {@const expanded = row.matchedThreads
          ? row.matchedThreads.length > 0
          : activeAgent || expandedAgents.has(a.id)}
        {@const threads = row.matchedThreads ?? (expanded ? agentThreads.slice(0, 8) : [])}
        <div>
          <div class="space-y-0.5">
            <!-- Clicking the agent = its working thread if one is live, else fresh. -->
            <RailRow active={activeAgent && agentSel?.conversationId === null} onClick={() => onOpenAgent(a.id)}>
              <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
              <span
                class="contents"
                oncontextmenu={(e) => menu.openMenu(e, [{ label: 'New thread', onSelect: () => onNewThread(a.id) }])}
              >
                <!-- R19: agents have no photo on the wire yet — initials. -->
                <PresenceAvatar name={a.label} presence={agentPresence(fleetStatus.get(a.id))} />
                <span class={cn('min-w-0 flex-1 truncate', unreadName(row.unread))}>{a.label}</span>
                {#if agentThreads.some((c) => c.working)}
                  <span class="gd-breathe h-1.5 w-1.5 shrink-0 rounded-full bg-accent" title="working on a reply"></span>
                {/if}
                {#if activeAgent && agentSel?.conversationId === null}
                  <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">new</span>
                {/if}
                <CountPill count={row.unread} />
                {#if agentThreads.length > 0 && !activeAgent && !row.matchedThreads}
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -- reason: pointer-only peek at an agent's threads; the RailRow button carries the row's click + keyboard -->
                  <span
                    role="button"
                    title={expanded ? 'Hide threads' : `Show threads (${agentThreads.length})`}
                    class="shrink-0 rounded px-0.5 text-[10px] text-muted hover:text-fg"
                    onclick={(e) => {
                      e.stopPropagation()
                      toggleExpanded(a.id)
                    }}
                  >
                    {expanded ? '▾' : '▸'}
                  </span>
                {/if}
              </span>
            </RailRow>
            {#each threads as c (c.id)}
              {@const activeThread = activeAgent && agentSel?.conversationId === c.id}
              <RailRow
                active={activeThread}
                onClick={() => onSelect({ t: 'agent', model: a.id, conversationId: c.id })}
                class={cn('pl-7', unreadName(c.unreadCount))}
              >
                <!-- svelte-ignore a11y_no_static_element_interactions -- reason: contextmenu is pointer-only; the RailRow button carries the row's click + keyboard -->
                <span class="contents" oncontextmenu={(e) => menu.openMenu(e, threadRowMenu(a.id, c))}>
                  <!-- §10 session-row anatomy, shared with the Plan rail. -->
                  <SessionRowBody conv={c} active={activeThread} />
                </span>
              </RailRow>
            {/each}
          </div>
        </div>
      {/each}
      {#if fleet.length === 0 && !view.searching}
        {#if fleetFailed}
          <RailFailure error={fleetQuery.error} title="Could not load your agents" onRetry={() => void fleetQuery.refetch()} />
        {:else if !fleetQuery.isLoading && !collapsed.dms}
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

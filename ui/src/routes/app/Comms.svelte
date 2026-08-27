<script lang="ts">
  import { route } from '@/router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import { CheckCheck, ClipboardList, Settings } from '@lucide/svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import GeneratingOverlay from '@/components/ui/GeneratingOverlay.svelte'
  import { alert, confirm, prompt } from '@/components/ui/confirm.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import RailSurface from '@/components/app/RailSurface.svelte'
  import Rail from '@/components/app/Rail.svelte'
  import RailRow from '@/components/app/RailRow.svelte'
  import CountPill from '@/components/app/CountPill.svelte'
  import ChannelView from '@/components/chat/ChannelView.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import ChannelSettingsModal from '@/components/chat/ChannelSettingsModal.svelte'
  import PlanModal from '@/components/chat/PlanModal.svelte'
  import { hydratePlanDraft } from '@/components/chat/plan-drafts.svelte'
  import { useAgents } from '@/lib/agents'
  import { errorMessage, getJson, patchJson, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { pushToast } from '@/lib/toast.svelte'
  import { useSession, useHasPerm } from '@/lib/session'
  import { useUsers } from '@/lib/users'
  import { useConversations } from '@/lib/conversations.svelte'
  import {
    commsSelectionFromPath,
    isCommsPath,
    readCommsSelection,
    restorableSelection,
    writeCommsSelection,
  } from '@/lib/comms-selection'
  import {
    addChannelAgent,
    addChannelMember,
    createChannel,
    markChannelRead,
    openDm,
    removeChannelAgent,
    removeChannelMember,
    useChannelDetail,
    useChannels,
    type Channel,
  } from '@/lib/channels.svelte'
  import Section from './comms/Section.svelte'
  import Hint from './comms/Hint.svelte'
  import RailFailure from './comms/RailFailure.svelte'
  import HeaderPicker from './comms/HeaderPicker.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import AgentDmPane from './comms/AgentDmPane.svelte'

  // Comms — every conversation in one place, Slack-shaped but agent-native:
  //   #channels  persistent, ambient (general talk, quick questions)
  //   Relays     named ad-hoc gatherings of people + agents around a purpose;
  //              they CONCLUDE (summary posted + indexed) and archive
  //   DMs        teammates (channel machinery) and agents (nested threads that
  //              distill into the activity brain and archive when idle)
  // Talking to an agent starts a NEW thread by default — bounded context per
  // topic, no giant-scrollback bloat riding along on every turn. Recent threads
  // nest under the agent in the sidebar for resuming deliberately.
  type Sel = { t: 'channel'; id: string } | { t: 'agent'; model: string; conversationId: string | null } | null

  const qc = useQueryClient()
  const session = useSession()
  // Every rail section below keeps its whole query, not just `data` — a
  // destructured `= []` throws the rejection away and the section then renders
  // its friendly "you have none yet" line over a 500. Four separate reads feed
  // this rail, so each owns its own failure marker (see `RailFailure`).
  const fleetQuery = useAgents()
  const fleet = $derived(fleetQuery.data?.agents ?? [])
  const channelsQuery = useChannels()
  const channels = $derived(channelsQuery.data ?? [])
  const usersQuery = useUsers()
  const users = $derived(usersQuery.data ?? [])
  const conversationsQuery = useConversations('chat')
  const conversations = $derived(conversationsQuery.data ?? [])

  // The URL IS the selection: ?c=<channel> or ?a=<agent>&x=<thread>. Every
  // pick navigates (push), so back/forward walks your reading order and any
  // view is copy-linkable.
  // ?c=<channelId> deep-links a channel/DM; ?a=<agentModel>&x=<convId>
  // deep-links an agent conversation (agent-outreach notifications land here).
  // ?t=agent (the /chat redirect) asks for the chat workspace: default to the
  // first agent's fresh thread instead of the first channel.
  // THE PATH IS THE SELECTION, and it is discriminated:
  //   /comms/channel/<id>
  //   /comms/agent/<model>[/<thread>]
  // The tag is a segment because the two kinds are different things, and a
  // one-segment id would be ambiguous between a channel and an agent model
  // until something resolved it.
  const sel = $derived(commsSelectionFromPath(route.pathname))
  // AM I STILL IN COMMS? `route.pathname` flips the instant you click a nav rail
  // item, but this component is not destroyed until afterwards — so for a beat
  // its effects run while the URL already points somewhere else. Every effect
  // below that NAVIGATES has to bail on this, or it fights the router. See
  // `isCommsPath` for the bug this fixes; it read as a dead nav rail.
  const onComms = $derived(isCommsPath(route.pathname))
  const searchT = $derived(searchParams.get('t') === 'agent' ? ('agent' as const) : undefined)
  const setSel = (next: Sel, opts: { replace?: boolean } = {}) => {
    const replace = opts.replace
    if (next?.t === 'channel') void navigate('/comms/channel/:id', { params: { id: next.id }, replace })
    else if (next?.t === 'agent' && next.conversationId)
      void navigate('/comms/agent/:model/:thread', { params: { model: next.model, thread: next.conversationId }, replace })
    else if (next?.t === 'agent') void navigate('/comms/agent/:model', { params: { model: next.model }, replace })
    else void navigate('/comms', { replace })
  }
  // The agent-flavored selection, pre-narrowed for the template.
  const agentSel = $derived(sel?.t === 'agent' ? sel : null)
  // Agents whose thread list is pinned open (chevron) without being selected.
  let expandedAgents = $state<Set<string>>(new Set())
  // Bumped on every deliberate fresh-thread start; drives ChatView's reset.
  let fresh = $state(0)
  let settingsOpen = $state(false)
  let planOpen = $state(false)

  const newThread = (model: string) => {
    setSel({ t: 'agent', model, conversationId: null })
    fresh += 1
  }

  // Clicking an agent lands on its WORKING thread when a reply is in flight —
  // leaving mid-stream and coming back must show the agent still at it, not a
  // blank new thread that makes the work look lost. Otherwise: fresh thread.
  const openAgent = (model: string) => {
    const working = conversations.find((c) => c.agentModel === model && c.working)
    if (working) setSel({ t: 'agent', model, conversationId: working.id })
    else newThread(model)
  }

  // Clicking an agent BEFORE the conversations query resolves can't see its
  // working thread — when the data first lands, upgrade a still-fresh agent
  // selection to that thread so in-flight work is resumed, not shadowed.
  // Once only (on the load transition): deliberate new threads afterwards
  // must never be yanked into the working thread.
  let upgradedOnLoad = false
  $effect(() => {
    if (conversationsQuery.isLoading || upgradedOnLoad) return
    upgradedOnLoad = true
    if (sel?.t !== 'agent' || sel.conversationId !== null) return
    const model = sel.model
    const working = conversations.find((c) => c.agentModel === model && c.working)
    if (working) setSel({ t: 'agent', model, conversationId: working.id }, { replace: true })
  })

  const rooms = $derived(channels.filter((c) => c.kind === 'channel'))
  const relays = $derived(channels.filter((c) => c.kind === 'group'))
  const dms = $derived(channels.filter((c) => c.kind === 'dm'))
  const people = $derived(users.filter((u) => u.id !== session.data?.id))
  const dmByPeer = $derived(new Map(dms.map((c) => [c.peer?.userId, c])))

  // REMEMBER WHAT IS SELECTED, so leaving Comms and coming back through the nav
  // rail does not land on the first channel. `sel` is derived from this view's
  // own search params and nothing else, so there is no moment at which the
  // value here disagrees with the URL — and a null `sel` (the transient empty
  // URL on the way out) writes nothing, so leaving cannot erase the memory.
  $effect(() => {
    if (sel) writeCommsSelection(sel)
  })

  // Default selection; heal a selection that vanished (archived). All are
  // replace-navigations — housekeeping shouldn't pollute history.
  // /chat lands here with ?t=agent: the chat workspace must be directly
  // reachable, so default to the first agent's fresh thread (§7 composer)
  // instead of the first channel; no agents → channel default as usual.
  $effect(() => {
    // On the way out this view still runs, and everything below navigates.
    // Answering "nothing is selected" about a page you are no longer on drags
    // the user back here; see `onComms`.
    if (!onComms) return
    if (!sel) {
      const saved = readCommsSelection()
      // Both rosters gate the restore, and only when the saved pick needs them:
      // validating against a list that has not arrived would discard a good
      // memory and fall straight through to the first channel.
      if (saved?.t === 'agent' && fleetQuery.isLoading) return
      if (channels.length === 0 && channelsQuery.isLoading) return
      const restorable = restorableSelection(saved, {
        channelIds: channels.map((c) => c.id),
        agentModels: fleet.map((a) => a.id),
        conversationIds: conversationsQuery.isLoading ? null : conversations.map((c) => c.id),
      })
      // An explicit ?t=agent (the /chat entry) asked for the chat workspace, so
      // a remembered CHANNEL does not get to answer it.
      if (restorable && (searchT !== 'agent' || restorable.t === 'agent')) {
        setSel(restorable, { replace: true })
        return
      }
      if (searchT === 'agent') {
        if (fleetQuery.isLoading) return
        if (fleet[0]) {
          setSel({ t: 'agent', model: fleet[0].id, conversationId: null }, { replace: true })
          return
        }
      }
      if (channels[0]) setSel({ t: 'channel', id: channels[0].id }, { replace: true })
      return
    }
    if (channels.length === 0 && channelsQuery.isLoading) return
    if (sel.t === 'channel' && channels.length > 0 && !channels.some((c) => c.id === sel.id)) {
      setSel(channels[0] ? { t: 'channel', id: channels[0].id } : null, { replace: true })
    }
  })

  const selected: Channel | null = $derived(
    sel?.t === 'channel' ? (channels.find((c) => c.id === sel.id) ?? null) : null,
  )
  const detailQuery = useChannelDetail(() => selected?.id ?? null)
  const detail = $derived(detailQuery.data)
  const refresh = () => qc.invalidateQueries({ queryKey: ['channels'] })

  // Ticket drafts pair to the channel SERVER-side: opening one asks what is
  // paired to it, so a reload lands back on an in-flight draft or a finished
  // review with nothing lost (the plan surface does the same for its kind).
  $effect(() => {
    const id = selected?.id
    if (!id) return
    void hydratePlanDraft(id, `/api/channels/${id}/plan`)
  })

  // Right-click menus on sidebar rows — shortcuts to actions the rows already
  // perform (open/copy the URL-driven selection, advance the read cursor).
  const menu = useContextMenu()

  // Sidebar "Mark read": the cursor advances to the channel's latest seq, and
  // only the messages list knows it — fetch it, post the cursor (the same call
  // ChannelView makes on open), then refresh the badges. Best-effort.
  const markRead = async (id: string) => {
    try {
      const { messages = [] } = await getJson<{ messages?: { seq: number }[] }>(`/api/channels/${id}/messages`)
      const latest = messages[messages.length - 1]?.seq ?? 0
      if (!latest) return
      await markChannelRead(id, latest)
      await qc.invalidateQueries({ queryKey: ['channels'] })
    } catch {
      // badges are advisory; a failed mark-read fixes itself on open
    }
  }

  const channelRowMenu = (c: Channel): ContextMenuEntry[] => [
    { label: 'Open', onSelect: () => setSel({ t: 'channel', id: c.id }) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/comms/channel/${c.id}`) },
    { label: 'Mark read', disabled: !c.unreadCount, onSelect: () => void markRead(c.id) },
  ]

  const mayCreateChannels = useHasPerm('comms.channels')
  const mayStartRelays = useHasPerm('comms.relays')
  const create = async (name: string, kind: 'channel' | 'group') => {
    const c = await createChannel(name, kind)
    await refresh()
    setSel({ t: 'channel', id: c.id })
    settingsOpen = true // straight into adding people/agents
  }

  const startDm = async (userId: string) => {
    const c = await openDm(userId)
    await refresh()
    setSel({ t: 'channel', id: c.id })
  }

  let concluding = $state(false)
  const conclude = async () => {
    if (!selected) return
    if (
      !(await confirm({
        title: 'Conclude relay',
        message: `Wrap up "${selected.name}"? A summary of what was decided is posted and indexed, then the relay archives.`,
        confirmLabel: 'Conclude',
      }))
    )
      return
    concluding = true
    try {
      const j = await postJson<{ summary?: string }>(`/api/channels/${selected.id}/conclude`)
      await refresh()
      void alert({ title: `${selected.name} concluded`, message: j.summary ?? 'Summarized and archived.' })
    } catch (e) {
      void alert({ title: 'Could not conclude', message: errorMessage(e) })
    } finally {
      concluding = false
    }
  }

  const peerLabel = (c: Channel) => c.peer?.name ?? c.peer?.email ?? 'teammate'
  const title = $derived(selected ? (selected.kind === 'dm' ? peerLabel(selected) : selected.name) : '')

  const toggleExpanded = (id: string) => {
    const next = new Set(expandedAgents)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    expandedAgents = next
  }

  // Silhouette widths for the rail-row skeletons — varied per index so the
  // sketch doesn't look stamped.
  const railW = ['w-24', 'w-32', 'w-20', 'w-28']
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

<RailSurface>
  <Rail>
    <Section
      label="Channels"
      meta={rooms.length > 0 ? String(rooms.length).padStart(2, '0') : undefined}
      createPlaceholder="channel name"
      onCreate={mayCreateChannels.current ? (v) => void create(v, 'channel') : undefined}
      loading={channelsQuery.isLoading}
      count={4}
      rowSkeleton={glyphRowSkeleton}
    >
      {#each rooms as c (c.id)}
        <RailRow active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
          <!-- display:contents wrapper — carries the context menu without touching row layout -->
          <span class="contents" oncontextmenu={(e) => menu.openMenu(e, channelRowMenu(c))}>
            <span class="shrink-0 opacity-60">#</span>
            <span class="min-w-0 flex-1 truncate">{c.name}</span>
            <CountPill count={c.unreadCount} />
          </span>
        </RailRow>
      {/each}
      {#if rooms.length === 0}
        {#if channelsQuery.isError && channelsQuery.data === undefined}
          <RailFailure
            error={channelsQuery.error}
            title="Could not load channels"
            onRetry={() => void channelsQuery.refetch()}
          />
        {:else}
          <Hint>Ambient, persistent talk.</Hint>
        {/if}
      {/if}
    </Section>

    <Section
      label="Relays"
      meta={relays.length > 0 ? String(relays.length).padStart(2, '0') : undefined}
      createPlaceholder="what's it about?"
      onCreate={mayStartRelays.current ? (v) => void create(v, 'group') : undefined}
      loading={channelsQuery.isLoading}
      count={3}
      rowSkeleton={glyphRowSkeleton}
    >
      {#each relays as c (c.id)}
        <RailRow active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
          <span class="contents" oncontextmenu={(e) => menu.openMenu(e, channelRowMenu(c))}>
            <span class="shrink-0 opacity-60">⇄</span>
            <span class="min-w-0 flex-1 truncate">{c.name}</span>
            <CountPill count={c.unreadCount} />
          </span>
        </RailRow>
      {/each}
      {#if relays.length === 0}
        {#if channelsQuery.isError && channelsQuery.data === undefined}
          <RailFailure
            error={channelsQuery.error}
            title="Could not load relays"
            onRetry={() => void channelsQuery.refetch()}
          />
        {:else}
          <Hint>Gather people + agents around a purpose; conclude when done.</Hint>
        {/if}
      {/if}
    </Section>

    <Section
      label="Teammates"
      meta={people.length > 0 ? String(people.length).padStart(2, '0') : undefined}
      loading={usersQuery.isLoading}
      count={4}
      rowSkeleton={avatarRowSkeleton}
    >
      {#each people as u (u.id)}
        {@const dm = dmByPeer.get(u.id)}
        <RailRow
          active={sel?.t === 'channel' && sel.id === dm?.id}
          onClick={() => (dm ? setSel({ t: 'channel', id: dm.id }) : void startDm(u.id))}
        >
          <span
            class="contents"
            oncontextmenu={(e) =>
              menu.openMenu(e, dm ? channelRowMenu(dm) : [{ label: 'Open', onSelect: () => void startDm(u.id) }])}
          >
            <Avatar name={u.name ?? u.email ?? '?'} class="h-5 w-5 shrink-0 text-[10px]" />
            <span class="min-w-0 flex-1 truncate">{u.name ?? u.email}</span>
            <CountPill count={dm?.unreadCount} />
          </span>
        </RailRow>
      {/each}
      {#if people.length === 0}
        {#if usersQuery.isError && usersQuery.data === undefined}
          <!-- "Just you so far." over a failed directory read is how a
               20-person org gets told it is one person. -->
          <RailFailure
            error={usersQuery.error}
            title="Could not load teammates"
            onRetry={() => void usersQuery.refetch()}
          />
        {:else}
          <Hint>Just you so far.</Hint>
        {/if}
      {/if}
    </Section>

    <Section
      label="Agents"
      meta={fleet.length > 0 ? String(fleet.length).padStart(2, '0') : undefined}
      loading={fleetQuery.isLoading}
      count={3}
      rowSkeleton={avatarRowSkeleton}
    >
      {#each fleet as a (a.id)}
        {@const activeAgent = agentSel?.model === a.id}
        {@const agentThreads = conversations.filter((c) => c.agentModel === a.id)}
        <!-- Threads unfold for the active agent, or via the chevron —
             peeking at an agent's threads shouldn't require selecting it. -->
        {@const expanded = activeAgent || expandedAgents.has(a.id)}
        {@const threads = expanded ? agentThreads.slice(0, 8) : []}
        <div>
          <div class="space-y-0.5">
            <!-- Clicking the agent = its working thread if one is live, else fresh. -->
            <RailRow active={activeAgent && agentSel?.conversationId === null} onClick={() => openAgent(a.id)}>
              <span
                class="contents"
                oncontextmenu={(e) => menu.openMenu(e, [{ label: 'New thread', onSelect: () => newThread(a.id) }])}
              >
                <span class="shrink-0 opacity-60">◍</span>
                <span class="min-w-0 flex-1 truncate">{a.label}</span>
                {#if conversations.some((c) => c.agentModel === a.id && c.working)}
                  <span class="gd-breathe h-1.5 w-1.5 shrink-0 rounded-full bg-accent" title="working on a reply"></span>
                {/if}
                {#if activeAgent && agentSel?.conversationId === null}
                  <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">new</span>
                {/if}
                {#if agentThreads.length > 0 && !activeAgent}
                  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
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
              <RailRow
                active={activeAgent && agentSel?.conversationId === c.id}
                onClick={() => setSel({ t: 'agent', model: a.id, conversationId: c.id })}
                class="pl-7"
              >
                <span
                  class="contents"
                  oncontextmenu={(e) =>
                    menu.openMenu(e, [
                      { label: 'Open', onSelect: () => setSel({ t: 'agent', model: a.id, conversationId: c.id }) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/comms/agent/${a.id}/${c.id}`) },
                      {
                        label: 'Rename',
                        onSelect: () => {
                          void prompt({ title: 'Rename thread', defaultValue: c.title ?? '', placeholder: 'Thread name', confirmLabel: 'Rename' }).then(async (name) => {
                            if (!name?.trim()) return
                            try {
                              await patchJson(`/api/conversations/${c.id}`, { title: name.trim() })
                            } catch (e) {
                              pushToast({ title: 'Rename failed', body: errorMessage(e), tone: 'danger' })
                            }
                            void qc.invalidateQueries({ queryKey: ['conversations'] })
                          })
                        },
                      },
                    ])}
                >
                  <!-- §10 session-row anatomy, shared with the Plan rail. -->
                  <SessionRowBody conv={c} active={activeAgent && agentSel?.conversationId === c.id} />
                </span>
              </RailRow>
            {/each}
          </div>
        </div>
      {/each}
      {#if fleet.length === 0}
        {#if fleetQuery.isError && fleetQuery.data === undefined}
          <RailFailure
            error={fleetQuery.error}
            title="Could not load your agents"
            onRetry={() => void fleetQuery.refetch()}
          />
        {:else}
          <Hint>No agents yet. Hire on /agents.</Hint>
        {/if}
      {/if}
      <!-- Threads nest under the agent rows above, so a failed conversation
           read makes every agent look like it has never been talked to.
           The rows stay (good fleet data is not thrown away) — the marker
           says the threads are missing rather than absent. -->
      {#if conversationsQuery.isError && conversationsQuery.data === undefined}
        <div transition:slide={{ duration: 150 }}>
          <RailFailure
            error={conversationsQuery.error}
            title="Could not load your threads"
            onRetry={() => void conversationsQuery.refetch()}
          />
        </div>
      {/if}
    </Section>
  </Rail>

  <main class="min-h-0 min-w-0 flex-1">
    {#if agentSel}
      {#key agentSel.model}
        <AgentDmPane
          model={agentSel.model}
          {fleet}
          conversationId={agentSel.conversationId}
          newChatSignal={fresh}
          onNewThread={() => agentSel && newThread(agentSel.model)}
          onCreated={(id) => {
            if (agentSel) setSel({ t: 'agent', model: agentSel.model, conversationId: id })
            void qc.invalidateQueries({ queryKey: ['conversations'] })
          }}
        />
      {/key}
    {:else if selected}
      <div class="flex h-full min-h-0 flex-col">
        <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
          <span class="text-sm font-semibold text-fg">
            {selected.kind === 'channel' ? `#${title}` : selected.kind === 'group' ? `⇄ ${title}` : title}
          </span>
          {#if selected.topic}<span class="truncate text-xs text-muted">{selected.topic}</span>{/if}
          <span class="ml-auto"></span>
          <!-- Pill-shaped placeholders while membership loads — the header
               holds its shape instead of the pickers popping in. -->
          {#if !detail && detailQuery.isLoading && selected.kind !== 'dm'}
            <Skeleton class="h-6 w-20 rounded-full" />
            <Skeleton class="h-6 w-20 rounded-full" />
          {/if}
          {#if detail && selected.kind !== 'dm'}
            {@const channelId = selected.id}
            <!-- Membership managed right here — the settings modal is for renames/danger. -->
            <HeaderPicker
              label={`${detail.members.length} ${detail.members.length === 1 ? 'person' : 'people'}`}
              options={users
                .filter((u) => u.email)
                .map((u) => ({
                  value: u.id,
                  label: u.name ?? u.email ?? u.id,
                  locked: detail.members.some((m) => m.userId === u.id && m.role === 'owner'),
                }))}
              selected={detail.members.map((m) => m.userId)}
              onToggle={async (id, on) => {
                const email = users.find((u) => u.id === id)?.email
                if (on && email) await addChannelMember(channelId, email)
                if (!on) await removeChannelMember(channelId, id)
                await qc.invalidateQueries({ queryKey: ['channel', channelId] })
              }}
            />
            <HeaderPicker
              label={`${detail.agents.length} ${detail.agents.length === 1 ? 'agent' : 'agents'}`}
              options={fleet.map((a) => ({ value: a.id, label: a.label }))}
              selected={detail.agents}
              onToggle={async (model, on) => {
                if (on) await addChannelAgent(channelId, model)
                else await removeChannelAgent(channelId, model)
                await qc.invalidateQueries({ queryKey: ['channel', channelId] })
              }}
            />
          {/if}
          {#if (detail?.agents.length ?? 0) > 0}
            <IconButton size="sm" title="Draft tickets from this conversation" onclick={() => (planOpen = true)}>
              <ClipboardList size={16} />
            </IconButton>
          {/if}
          {#if selected.kind === 'group'}
            <IconButton size="sm" title="Conclude: summarize what was decided, then archive" onclick={() => void conclude()}>
              <CheckCheck size={16} />
            </IconButton>
          {/if}
          {#if selected.kind !== 'dm'}
            <IconButton size="sm" title="Settings: people, agents, rename" onclick={() => (settingsOpen = true)}>
              <Settings size={16} />
            </IconButton>
          {/if}
        </header>
        <div class="relative min-h-0 flex-1">
          {#if concluding}<GeneratingOverlay site="comms/conclude" label="Concluding: summarizing what was decided, then archiving" />{/if}
          <!-- ChannelView fetches its own channel detail (svelte-query
               dedupes with the header's call) so the message pane renders
               independently of this fetch. -->
          {#key selected.id}
            <ChannelView channelId={selected.id} channelName={title} {fleet} />
          {/key}
        </div>
      </div>
    {:else}
      <EmptyState
        icon="◈"
        title="All your conversations, one place"
        hint="Channels for ambient talk, relays for deciding things with people and agents, DMs for anyone, human or agent."
      />
    {/if}
  </main>

  {#if selected && detail && planOpen}
    <PlanModal
      open={planOpen}
      onClose={() => (planOpen = false)}
      planId={selected.id}
      draftUrl={`/api/channels/${selected.id}/plan`}
      agents={fleet.filter((a) => detail.agents.includes(a.id))}
    />
  {/if}
  {#if selected && detail && selected.kind !== 'dm'}
    <ChannelSettingsModal
      open={settingsOpen}
      onClose={() => (settingsOpen = false)}
      channelId={selected.id}
      channelName={selected.name}
      {detail}
      {fleet}
      selfUserId={session.data?.id ?? null}
      onDeleted={() => {
        settingsOpen = false
        void refresh()
      }}
    />
  {/if}
  <ContextMenu {menu} />
</RailSurface>

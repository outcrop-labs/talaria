<script lang="ts">
  // The plan / work-session rail — LEFT, on the shared Rail primitives (agent
  // picker up top, the chosen agent's conversations below, ones shared with you
  // at the end). Rows follow the Mercury session-list pattern (spec §10): status
  // dot, 13px sans title, right-aligned mono meta/time; the active row carries
  // the gold dot.
  //
  // TWO SURFACES, ONE RAIL. Plan and Work differ only in what they call a row,
  // which permission opens the `+`, and — Work — whether the agent pick also
  // FILTERS and what a row is allowed to say about itself. All of that is props
  // rather than a second copy of this file that would drift from it: Research's
  // rail was that second copy, and the affordances it had alone (all agents, a
  // run's status and mode on the row, a run's own menu) are `onSelectAll`,
  // `rowMeta` and `rowMenu` below.
  //
  // THE RAIL IS ALSO THE VIEWER'S OWN FILING, and that half is this file's:
  // pinned rows lead, groups they filed things under collapse, the rest reads
  // as it always did. Which section a row lands in and what a drop has to
  // write are in `conversation-sidebar.ts` — pure, and tested there.
  import { MoreHorizontal, Plus } from '@lucide/svelte'
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useHasPerm } from '@/lib/session'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { flip, LIST, listStagger } from '@/lib/motion'
  import AgentPicker from '@/components/chat/AgentPicker.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import { prompt } from '@/components/ui/confirm.svelte'
  import GroupHeader from '@/components/app/GroupHeader.svelte'
  import Rail from '@/components/app/Rail.svelte'
  import RailRow from '@/components/app/RailRow.svelte'
  import RailSection from '@/components/app/RailSection.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import { readFlag, writeFlag } from '@/lib/persist'
  import type { DotStatus } from '@/components/ui/chip'
  import type { AgentModel } from '@/lib/agents'
  import { patchConversation, type Conversation } from '@/lib/conversations.svelte'
  import {
    layoutMenuItems,
    railGroups,
    railSections,
    reorderPosition,
    type PositionWrite,
    type RailSection as RailSectionShape,
    type SidebarFailure,
  } from './conversation-sidebar'

  let {
    agents,
    conversations,
    archived = [],
    selectedAgent,
    selectedConversationId,
    agentsLoading,
    conversationsLoading,
    agentsFailure = null,
    conversationsFailure = null,
    archivedFailure = null,
    onSelectAgent,
    onSelectConversation,
    onNewChat,
    onRowMenu,
    onSelectAll,
    rowMeta,
    rowMenu,
    noun = 'plan',
    newPerm = 'plans.create',
    newTitle = 'New plan: think it through, then draft tickets',
    emptyHint,
    collapseKey,
    class: className,
  }: {
    agents: AgentModel[]
    conversations: Conversation[]
    /** Retired rows — the way back after Archive. Same membership as the live list. */
    archived?: Conversation[]
    selectedAgent: string | null
    selectedConversationId: string | null
    agentsLoading?: boolean
    conversationsLoading?: boolean
    agentsFailure?: SidebarFailure
    conversationsFailure?: SidebarFailure
    archivedFailure?: SidebarFailure
    onSelectAgent: (agentModel: string) => void
    onSelectConversation: (conv: Conversation) => void
    onNewChat: () => void
    /** The fourth argument is the rail's own pin/file entries for that row —
     *  empty for an archived one — so a host menu can mirror them. */
    onRowMenu?: (e: MouseEvent, conv: Conversation, archived: boolean, layout?: ContextMenuEntry[]) => void
    /** Offer "all agents" in the picker. Given, the pick is a FILTER as well
     *  as a choice, and `selectedAgent === null` means every agent rather than
     *  "none chosen yet" — so the rail stops filtering and lists the lot.
     *  Withheld (Plan's rail), a null agent still means nothing to list, which
     *  is what the zero state says. Creating needs a concrete agent either
     *  way, so the `+` stays disabled on "all". */
    onSelectAll?: () => void
    /** Per-row decoration: state of the row's own and/or a kind, both drawn by
     *  SessionRowBody. A surface whose rows are not all plain sessions —
     *  research-origin work sessions carry their run's status and mode — says
     *  so here, instead of teaching this rail what a run is. Null for a row
     *  with nothing extra to say, which is today's plain row. */
    rowMeta?: (conv: Conversation) => { status?: DotStatus; pulse?: boolean; chip?: string } | null
    /** Takes precedence over `onRowMenu`, for a caller that hands some rows a
     *  different menu than its own (a research-origin row gets the run's). */
    rowMenu?: (e: MouseEvent, conv: Conversation, archived: boolean, layout?: ContextMenuEntry[]) => void
    /** What a row IS, for the rail's own sentences ("No plans yet", "Could not
     *  load your plans"). Plural is this + 's' — both nouns take it. */
    noun?: string
    /** The permission id that opens the `+`. */
    newPerm?: string
    /** The `+` button's tooltip — the surface says what it is for. */
    newTitle?: string
    /** What this surface IS, for the zero state. A rail that has never held a
     *  row is the first thing a new person sees on the surface, so it gets a
     *  sentence about what will live here rather than a bare "No plans yet".
     *  Omitted, the zero state keeps the title and drops the hint. */
    emptyHint?: string
    /** Opt this rail into collapsing (see Rail.svelte); the key is where the
     *  preference is remembered. */
    collapseKey?: string
    class?: string
  } = $props()

  // A getter, not the bare prop: `newPerm` arrives as a prop, and reading it
  // once at init is both the `state_referenced_locally` warning and a real
  // staleness if a caller ever swaps it.
  const canCreate = useHasPerm(() => newPerm)
  // `onSelectAll` is what turns a null agent into "all" (see the prop): only a
  // rail that OFFERS the entry can be sitting on it, and without it null is a
  // pick that hasn't landed yet — the agents read still in flight — where
  // listing everything would flash the whole workspace for a beat.
  const allAgents = $derived(onSelectAll != null && selectedAgent === null)
  const visibleConvs = $derived(
    allAgents ? conversations : conversations.filter((c) => c.agentModel === selectedAgent),
  )
  // Nothing is "elsewhere" when the rail is showing every agent — the rows
  // this section would lift out are already above it.
  const sharedElsewhere = $derived(
    allAgents
      ? []
      : conversations.filter((c) => c.role === 'collaborator' && c.agentModel !== selectedAgent),
  )
  // Collapsed until asked. An archived plan is not the work; it is the way back.
  let archivedOpen = $state(false)

  // ── The viewer's own filing ────────────────────────────────────────────────
  const sections = $derived(railSections(visibleConvs))
  const groups = $derived(railGroups(visibleConvs))

  // WHICH GROUPS ARE OPEN, per rail and per group — `collapseKey` already names
  // this surface's preferences (Rail's own collapse uses it), so the flag hangs
  // off it rather than inventing a second idea of what rail this is.
  const groupFlagKey = (group: string) => `talaria:session-group:${collapseKey ?? noun}:${group}`
  // Default open, and hydrated on the client: a group's flag has to be READ
  // before the first paint can honour it, and the store is not reachable
  // during SSR. Unknown groups answer "open" until the store says otherwise.
  let groupOpen = $state<Record<string, boolean>>({})
  $effect(() => {
    const names = sections.map((s) => s.group).filter((g): g is string => g !== null)
    // Untracked: this effect WRITES the map it would otherwise re-read, and a
    // group's flag is read once — the toggle below owns it from then on.
    untrack(() => {
      for (const g of names) if (!(g in groupOpen)) groupOpen[g] = readFlag(groupFlagKey(g), true)
    })
  })
  const isOpen = (group: string) => groupOpen[group] ?? true
  const toggleGroup = (group: string) => {
    const next = !isOpen(group)
    groupOpen[group] = next
    writeFlag(groupFlagKey(group), next)
  }

  const qc = useQueryClient()
  const menu = useContextMenu()

  type LayoutPatch = { pinned?: boolean; group?: string | null; position?: number | null }

  /** The row moves under the cursor, before the write lands. Every
   *  `['conversations', …]` cache at once, because a session can be listed by
   *  more than one of them (the merged 'sessions' set and its own kind) and a
   *  row that jumps in one rail and not the other is worse than no optimism. */
  const patchLocal = (id: string, patch: LayoutPatch) =>
    qc.setQueriesData<Conversation[]>({ queryKey: ['conversations'] }, (list) =>
      list ? list.map((c) => (c.id === id ? { ...c, ...patch } : c)) : list,
    )

  /** Settle on the server's answer, whichever way the write went — a refused
   *  PATCH has to take the optimistic row back with it. */
  const settle = () => void qc.invalidateQueries({ queryKey: ['conversations'] })

  const writeLayout = async (id: string, patch: LayoutPatch) => {
    patchLocal(id, patch)
    await patchConversation(id, patch)
    settle()
  }

  /** Pinning moves a row across sections exactly as filing does, so it gives
   *  up its slot the same way: a position carried from the ungrouped tail
   *  would land the row at an arbitrary height inside Pinned. */
  const togglePin = (c: Conversation) => void writeLayout(c.id, { pinned: !c.pinned, position: null })

  /** Filing a thread GIVES UP ITS SLOT: a position is a place within one
   *  section, so carrying it across would drop the row at an arbitrary height
   *  in its new group. Cleared, it lands by recency — where a thread nobody
   *  has placed belongs. */
  const moveToGroup = (c: Conversation, group: string | null) =>
    void writeLayout(c.id, { group, position: null })

  const askForGroup = async (c: Conversation) => {
    const name = await prompt({
      title: 'Move to group',
      placeholder: 'Group name',
      confirmLabel: 'Move',
    })
    const next = name?.trim()
    if (next) moveToGroup(c, next)
  }

  const layoutItemsFor = (c: Conversation) =>
    layoutMenuItems(c, groups, {
      pin: () => togglePin(c),
      moveTo: (group) => moveToGroup(c, group),
      newGroup: () => void askForGroup(c),
      ungroup: () => moveToGroup(c, null),
    })
  const openLayoutMenu = (e: MouseEvent, c: Conversation) => menu.openMenu(e, layoutItemsFor(c))

  // ── Drag to reorder, drag onto a header to file ────────────────────────────
  // The hand-rolled HTML5 pattern the boards sublist uses: the dragged row is
  // a state variable rather than dataTransfer payload, because `dragover` may
  // only inspect `types` and the rail needs to know WHICH row to draw against.
  let dragging = $state<Conversation | null>(null)
  /** Reorder stays inside one section. A row dragged across sections would
   *  have to mean "and change your group/pin too", and the header below — plus
   *  the row menu — already say that out loud. */
  let dragFrom = $state<string | null>(null)
  let overRow = $state<string | null>(null)
  let overGroup = $state<string | null>(null)

  const endDrag = () => {
    dragging = null
    dragFrom = null
    overRow = null
    overGroup = null
  }

  const canDropOn = (section: RailSectionShape, c: Conversation) =>
    dragging !== null && dragFrom === section.key && dragging.id !== c.id

  /** Which edge the indicator draws on — the same rule the drop itself
   *  follows: dragging down lands after the target, dragging up before it. */
  const dropEdge = (section: RailSectionShape, c: Conversation): 'above' | 'below' => {
    const from = section.items.findIndex((i) => i.id === dragging?.id)
    const to = section.items.findIndex((i) => i.id === c.id)
    return from < to ? 'below' : 'above'
  }

  const applyWrites = async (writes: PositionWrite[]) => {
    if (writes.length === 0) return
    for (const w of writes) patchLocal(w.id, { position: w.position })
    // One PATCH per row that moved — which is one row, unless the list had no
    // gap left and had to be renumbered.
    await Promise.all(writes.map((w) => patchConversation(w.id, { position: w.position })))
    settle()
  }

  const dropOnRow = (section: RailSectionShape, c: Conversation) => {
    const moved = dragging
    endDrag()
    if (!moved) return
    void applyWrites(reorderPosition(section.items, moved.id, c.id))
  }

  const dropOnGroup = (group: string) => {
    const moved = dragging
    endDrag()
    if (!moved || moved.pinned || (moved.group ?? null) === group) return
    moveToGroup(moved, group)
  }

  // The hover actions stand where the meta does, so the meta steps aside.
  const hideOnHover = 'group-focus-within/row:invisible group-hover/row:invisible'
</script>

<!-- ONE ROW, THREE LISTS (the listed ones, shared with you, archived). The
     decoration and the menu override have to be identical in all three, so the
     row is written once; `isArchived` is the only thing that differs. -->
{#snippet row(c: Conversation, isArchived: boolean, section: RailSectionShape | null)}
  {@const meta = rowMeta?.(c) ?? null}
  {@const over = section !== null && overRow === c.id && canDropOn(section, c)}
  <!-- svelte-ignore a11y_no_static_element_interactions -- reason: drag and contextmenu are pointer-only; the RailRow button inside carries the row's click + keyboard -->
  <div
    role="presentation"
    class={cn(
      'group/row relative rounded-md',
      section !== null && 'cursor-grab active:cursor-grabbing',
      dragging?.id === c.id && 'opacity-40',
    )}
    draggable={section !== null}
    ondragstart={(e) => {
      if (section === null) return
      dragging = c
      dragFrom = section.key
      if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
    }}
    ondragend={endDrag}
    ondragover={(e) => {
      if (section === null || !canDropOn(section, c)) return
      // Only a prevented dragover makes this a drop target at all.
      e.preventDefault()
      overRow = c.id
    }}
    ondragleave={() => {
      if (overRow === c.id) overRow = null
    }}
    ondrop={(e) => {
      // A row this drag cannot land on does NOT swallow the drop: it bubbles
      // to the group box around it, so letting go anywhere inside a group
      // files the thread there.
      if (section === null || !canDropOn(section, c)) return
      e.preventDefault()
      dropOnRow(section, c)
    }}
  >
    {#if over}
      <!-- The insertion line, on the edge the row will land against. -->
      <div
        aria-hidden="true"
        class={cn(
          'pointer-events-none absolute inset-x-1 z-10 h-px bg-accent',
          dropEdge(section, c) === 'above' ? '-top-px' : '-bottom-px',
        )}
      ></div>
    {/if}
    <RailRow
      active={c.id === selectedConversationId}
      onClick={() => onSelectConversation(c)}
      oncontextmenu={(e) => (rowMenu ?? onRowMenu)?.(e, c, isArchived, isArchived ? [] : layoutItemsFor(c))}
    >
      <SessionRowBody
        conv={c}
        active={c.id === selectedConversationId}
        pinned={section?.key === 'pinned'}
        metaClass={!isArchived ? hideOnHover : undefined}
        status={meta?.status}
        pulse={meta?.pulse}
        chip={meta?.chip}
      />
    </RailRow>
    {#if !isArchived}
      <!-- PIN AND FILE LIVE HERE, on every row the viewer can keep — a thread
           shared with them included; the layout is the viewer's own. The
           host's right-click menu mirrors these through `onRowMenu`'s fourth
           argument. -->
      <div class="absolute inset-y-0 right-1 hidden items-center group-focus-within/row:flex group-hover/row:flex">
        <button
          type="button"
          title={`Pin or file "${c.title || 'Untitled'}"`}
          aria-label={`Pin or file "${c.title || 'Untitled'}"`}
          onclick={(e) => {
            e.stopPropagation()
            openLayoutMenu(e, c)
          }}
          class="grid h-6 w-6 place-items-center rounded text-muted transition-colors hover:text-fg focus-visible:text-fg focus-visible:outline-none"
        >
          <MoreHorizontal size={13} />
        </button>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet startAction()}
  <Button size="sm" onclick={onNewChat}>
    <Plus size={14} />
    New {noun}
  </Button>
{/snippet}

<Rail {collapseKey} collapsedLabel={`${noun}s`} class={className}>
  {#snippet actions()}
    {#if canCreate.current}
      <IconButton size="sm" title={newTitle} onclick={onNewChat} disabled={!selectedAgent}><Plus size={15} /></IconButton>
    {/if}
  {/snippet}

  <div class="mb-3">
    <AgentPicker {agents} value={selectedAgent} {onSelectAll} onChange={onSelectAgent} loading={agentsLoading} fullWidth />
    <!-- An empty picker is indistinguishable from a fleet you don't have. -->
    {#if agentsFailure}
      <QueryError
        variant="inline"
        class="mt-1.5 px-2"
        error={agentsFailure.error}
        title="Could not load your agents"
        onRetry={agentsFailure.retry}
      />
    {/if}
  </div>

  {#if conversationsLoading}
    <SkeletonRows rows={5} class="px-2 pt-1.5" />
  {:else if conversationsFailure}
    <QueryError
      variant="compact"
      error={conversationsFailure.error}
      title={`Could not load your ${noun}s`}
      onRetry={conversationsFailure.retry}
    />
  {:else if visibleConvs.length === 0}
    <!-- THE FIRST THING A NEW PERSON SEES on this surface, so it says what
         will live here and offers the one action that makes it happen — a
         bare "No plans yet with this agent." was a dead end that named the
         absence and nothing else. The sentence stays agent-free for exactly
         that reason, and on "all agents" it is also the literal truth: no
         sessions, by anyone. The action waits for a concrete agent, because
         that is what creating one needs. `compact`: this sits inside a rail
         that already has texture, not in a pane of its own. -->
    <EmptyState variant="compact" icon="◇" title={`No ${noun}s yet`} hint={emptyHint} action={canCreate.current && selectedAgent ? startAction : undefined} />
  {:else}
    <!-- THE ORGANISED RAIL: pinned first, then each group the viewer filed
         something under, then everything else — which is the rail as it always
         looked, bare and unlabelled. Nothing is nested under a header a person
         did not create. -->
    {#each sections as section (section.key)}
      {#if section.key === 'pinned'}
        <RailSection label="Pinned">
          {#each section.items as c (c.id)}
            <div animate:flip={LIST}>{@render row(c, false, section)}</div>
          {/each}
        </RailSection>
      {:else if section.group !== null}
        {@const group = section.group}
        {@const open = isOpen(group) || dragging !== null}
        <!-- svelte-ignore a11y_no_static_element_interactions -- reason: the drop target is pointer-only; the header's own button carries click + keyboard -->
        <div
          class={cn(
            'mb-4 rounded-md',
            dragging && overGroup === group && 'bg-raised ring-1 ring-[color:var(--theme-accent-border)]',
          )}
          ondragover={(e) => {
            // A drag force-opens every group (below) so there is always
            // somewhere visible to drop; the header itself takes the row.
            // A pinned row lives under Pinned whatever group it carries, so a
            // group header is no target for it: offering the drop and then
            // moving nothing would be a lie.
            if (!dragging || dragging.pinned || (dragging.group ?? null) === group) return
            e.preventDefault()
            overGroup = group
          }}
          ondragleave={() => {
            if (overGroup === group) overGroup = null
          }}
          ondrop={(e) => {
            e.preventDefault()
            dropOnGroup(group)
          }}
        >
          <GroupHeader
            label={group}
            count={section.items.length}
            {open}
            onToggle={() => toggleGroup(group)}
          />
          {#if open}
            <div class="space-y-0.5" use:listStagger>
              {#each section.items as c (c.id)}
                <div animate:flip={LIST}>{@render row(c, false, section)}</div>
              {/each}
            </div>
          {/if}
        </div>
      {:else}
        <!-- div, not ul: RailRow renders div rows. -->
        <div class="space-y-0.5" use:listStagger>
          {#each section.items as c (c.id)}
            <div animate:flip={LIST}>{@render row(c, false, section)}</div>
          {/each}
        </div>
      {/if}
    {/each}
  {/if}

  <!-- Rows shared WITH you ride other agents — always visible. -->
  {#if sharedElsewhere.length > 0}
    <div class="mt-4">
      <RailSection label="Shared with you">
        {#each sharedElsewhere as c (c.id)}
          {@render row(c, false, null)}
        {/each}
      </RailSection>
    </div>
  {/if}
  {#if archivedFailure}
    <QueryError
      variant="inline"
      class="mt-3 px-2"
      error={archivedFailure.error}
      title="Could not load archived"
      onRetry={archivedFailure.retry}
    />
  {/if}
  {#if archived.length > 0}
    <div class="mt-4">
      <GroupHeader label="Archived" count={archived.length} open={archivedOpen} onToggle={() => (archivedOpen = !archivedOpen)} />
      {#if archivedOpen}
        <div class="space-y-0.5">
          {#each archived as c (c.id)}
            {@render row(c, true, null)}
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  <!-- The rail's own menu — pin and file. The host's record menu is its own
       controller and opens on right-click; this one only ever opens from the
       row's hover control, so the two never collide. -->
  <ContextMenu {menu} />
</Rail>

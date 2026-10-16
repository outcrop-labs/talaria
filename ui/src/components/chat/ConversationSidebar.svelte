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
  import { Plus } from '@lucide/svelte'
  import { useHasPerm } from '@/lib/session'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { listStagger } from '@/lib/motion'
  import AgentPicker from '@/components/chat/AgentPicker.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import GroupHeader from '@/components/app/GroupHeader.svelte'
  import Rail from '@/components/app/Rail.svelte'
  import RailRow from '@/components/app/RailRow.svelte'
  import RailSection from '@/components/app/RailSection.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import type { AgentModel } from '@/lib/agents'
  import type { Conversation } from '@/lib/conversations.svelte'
  import type { SidebarFailure } from './conversation-sidebar'

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
    onRowMenu?: (e: MouseEvent, conv: Conversation, archived: boolean) => void
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
    rowMenu?: (e: MouseEvent, conv: Conversation, archived: boolean) => void
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
</script>

<!-- ONE ROW, THREE LISTS (the listed ones, shared with you, archived). The
     decoration and the menu override have to be identical in all three, so the
     row is written once; `isArchived` is the only thing that differs. -->
{#snippet row(c: Conversation, isArchived: boolean)}
  {@const meta = rowMeta?.(c) ?? null}
  <RailRow
    active={c.id === selectedConversationId}
    onClick={() => onSelectConversation(c)}
    oncontextmenu={(e) => (rowMenu ?? onRowMenu)?.(e, c, isArchived)}
  >
    <SessionRowBody
      conv={c}
      active={c.id === selectedConversationId}
      status={meta?.status}
      pulse={meta?.pulse}
      chip={meta?.chip}
    />
  </RailRow>
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
    <!-- div, not ul: RailRow renders div rows. -->
    <div class="space-y-0.5" use:listStagger>
      {#each visibleConvs as c (c.id)}
        {@render row(c, false)}
      {/each}
    </div>
  {/if}

  <!-- Rows shared WITH you ride other agents — always visible. -->
  {#if sharedElsewhere.length > 0}
    <div class="mt-4">
      <RailSection label="Shared with you">
        {#each sharedElsewhere as c (c.id)}
          {@render row(c, false)}
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
            {@render row(c, true)}
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</Rail>

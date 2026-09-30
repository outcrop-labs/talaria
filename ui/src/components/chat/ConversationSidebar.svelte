<script lang="ts">
  // The plan / work-session rail — LEFT, on the shared Rail primitives (agent
  // picker up top, this agent's conversations below, ones shared with you at
  // the end). Rows follow the Mercury session-list pattern (spec §10): status
  // dot, 13px sans title, right-aligned mono meta/time; the active row carries
  // the gold dot.
  //
  // TWO SURFACES, ONE RAIL. Plan and Work differ only in what they call a row
  // and which permission opens the `+`, so those are props rather than a
  // second copy of this file that would drift from it.
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
    /** What a row IS, for the rail's own sentences ("No plans yet with this
     *  agent."). Plural is this + 's' — both nouns take it. */
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
  const agentConvs = $derived(conversations.filter((c) => c.agentModel === selectedAgent))
  const sharedElsewhere = $derived(
    conversations.filter((c) => c.role === 'collaborator' && c.agentModel !== selectedAgent),
  )
  // Collapsed until asked. An archived plan is not the work; it is the way back.
  let archivedOpen = $state(false)
</script>

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
    <AgentPicker {agents} value={selectedAgent} onChange={onSelectAgent} loading={agentsLoading} fullWidth />
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
  {:else if agentConvs.length === 0}
    <!-- THE FIRST THING A NEW PERSON SEES on this surface, so it says what
         will live here and offers the one action that makes it happen — a
         bare "No plans yet with this agent." was a dead end that named the
         absence and nothing else. `compact`: this sits inside a rail that
         already has texture, not in a pane of its own. -->
    <EmptyState variant="compact" icon="◇" title={`No ${noun}s yet`} hint={emptyHint} action={canCreate.current && selectedAgent ? startAction : undefined} />
  {:else}
    <!-- div, not ul: RailRow renders div rows. -->
    <div class="space-y-0.5" use:listStagger>
      {#each agentConvs as c (c.id)}
        <RailRow active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)} oncontextmenu={(e) => onRowMenu?.(e, c, false)}>
          <SessionRowBody conv={c} active={c.id === selectedConversationId} />
        </RailRow>
      {/each}
    </div>
  {/if}

  <!-- Rows shared WITH you ride other agents — always visible. -->
  {#if sharedElsewhere.length > 0}
    <div class="mt-4">
      <RailSection label="Shared with you">
        {#each sharedElsewhere as c (c.id)}
          <RailRow active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)} oncontextmenu={(e) => onRowMenu?.(e, c, false)}>
            <SessionRowBody conv={c} active={c.id === selectedConversationId} />
          </RailRow>
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
            <RailRow active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)} oncontextmenu={(e) => onRowMenu?.(e, c, true)}>
              <SessionRowBody conv={c} active={c.id === selectedConversationId} />
            </RailRow>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</Rail>

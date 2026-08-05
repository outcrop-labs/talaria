<script lang="ts">
  // The plan rail — LEFT, on the shared Rail primitives (agent picker up top,
  // this agent's plans below, plans shared with you at the end). Rows follow
  // the Mercury session-list pattern (spec §10): status dot, 13px sans
  // title, right-aligned mono meta/time; the active row carries the gold dot.
  import { Plus } from '@lucide/svelte'
  import { useHasPerm } from '@/lib/session'
  import AgentPicker from '@/components/chat/AgentPicker.svelte'
  import SessionRowBody from '@/components/chat/SessionRowBody.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
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
    selectedAgent,
    selectedConversationId,
    agentsLoading,
    conversationsLoading,
    agentsFailure = null,
    conversationsFailure = null,
    onSelectAgent,
    onSelectConversation,
    onNewChat,
  }: {
    agents: AgentModel[]
    conversations: Conversation[]
    selectedAgent: string | null
    selectedConversationId: string | null
    agentsLoading?: boolean
    conversationsLoading?: boolean
    agentsFailure?: SidebarFailure
    conversationsFailure?: SidebarFailure
    onSelectAgent: (agentModel: string) => void
    onSelectConversation: (conv: Conversation) => void
    onNewChat: () => void
  } = $props()

  const canCreatePlans = useHasPerm('plans.create')
  const agentConvs = $derived(conversations.filter((c) => c.agentModel === selectedAgent))
  const sharedElsewhere = $derived(
    conversations.filter((c) => c.role === 'collaborator' && c.agentModel !== selectedAgent),
  )
</script>

<Rail title="Plan">
  {#snippet actions()}
    {#if canCreatePlans.current}
      <IconButton size="sm" title="New plan: think it through, then draft tickets" onclick={onNewChat} disabled={!selectedAgent}><Plus size={15} /></IconButton>
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
      title="Could not load your plans"
      onRetry={conversationsFailure.retry}
    />
  {:else if agentConvs.length === 0}
    <div class="px-2 py-6 text-center text-xs text-muted">No plans yet with this agent.</div>
  {:else}
    <ul class="space-y-0.5">
      {#each agentConvs as c (c.id)}
        <RailRow active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)}>
          <SessionRowBody conv={c} active={c.id === selectedConversationId} />
        </RailRow>
      {/each}
    </ul>
  {/if}

  <!-- Plans shared WITH you ride other agents — always visible. -->
  {#if sharedElsewhere.length > 0}
    <div class="mt-4">
      <RailSection label="Shared with you">
        {#each sharedElsewhere as c (c.id)}
          <RailRow active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)}>
            <SessionRowBody conv={c} active={c.id === selectedConversationId} />
          </RailRow>
        {/each}
      </RailSection>
    </div>
  {/if}
</Rail>

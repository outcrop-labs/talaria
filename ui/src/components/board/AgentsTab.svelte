<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useAgents } from '@/lib/agents'
  import {
    decideBoardAgentRequest,
    setBoardAgents,
    useBoardAgentRequests,
    useBoardAgents,
    type Board,
  } from '@/lib/boards.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { relativeTime } from '@/lib/fleet'

  // The Agents tab of BoardSettingsModal.svelte.
  let { board, canEdit }: { board: Board; canEdit: boolean } = $props()

  const qc = useQueryClient()
  const fleetQuery = useAgents()
  const fleet = $derived(fleetQuery.data)
  const fleetLoading = $derived(fleetQuery.isLoading)
  const options = $derived((fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role })))
  // The checkbox and the picker are SEEDED from `config`. On rejection the
  // effect below never runs, so the panel renders "Allow all agents" unchecked
  // with an empty selection — a maximally restrictive policy invented from a
  // read that failed, and one Save away from becoming real.
  const configQuery = useBoardAgents(() => board.id)
  const config = $derived(configQuery.data)
  const configLoading = $derived(configQuery.isLoading)
  const configFailed = $derived(
    (configQuery.isError && config === undefined) || (fleetQuery.isError && fleet === undefined),
  )
  // The request queue is editor surface: the GET gates on edit-or-elevated
  // server-side, so a viewer never even asks.
  const requestsQuery = useBoardAgentRequests(() => board.id, canEdit)
  const requests = $derived(requestsQuery.data ?? [])
  let allowAll = $state(false)
  let agents = $state<string[]>([])
  let busy = $state(false)
  let saved = $state(false)
  let deciding = $state<string | null>(null)

  $effect(() => {
    if (config) {
      allowAll = config.allowAll
      agents = config.models
    }
  })

  const save = async () => {
    busy = true
    try {
      await setBoardAgents(board.id, allowAll, allowAll ? [] : agents)
      await qc.invalidateQueries({ queryKey: ['board-agents', board.id] })
      saved = true
      setTimeout(() => (saved = false), 1500)
    } finally {
      busy = false
    }
  }

  const decide = (agentModel: string, agentLabel: string, action: 'approve' | 'reject') => {
    deciding = agentModel
    decideBoardAgentRequest(board.id, agentModel, action)
      .then(async () => {
        await qc.invalidateQueries({ queryKey: ['board-agent-requests', board.id] })
        // Approve rewrites the policy the tab below is showing.
        await qc.invalidateQueries({ queryKey: ['board-agents', board.id] })
      })
      .catch((e) =>
        pushToast({
          title: action === 'approve' ? 'Could not approve request' : 'Could not decline request',
          body: `${agentLabel}: ${errorMessage(e)}`,
          tone: 'danger',
        }),
      )
      .finally(() => (deciding = null))
  }
</script>

<div>
  {#if canEdit && requests.length > 0}
    <div class="mb-4 space-y-1">
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Agents asking to join
      </div>
      {#each requests as r (r.id)}
        <div class="flex items-center gap-2 rounded-md border border-line px-2 py-1.5">
          <div class="min-w-0 flex-1">
            <div class="truncate font-sans text-sm text-fg">
              {r.agentName ?? r.agentModel}
              <span class="ml-1.5 font-mono text-[11px] text-muted">{r.agentModel}</span>
            </div>
            <div class="truncate font-sans text-xs text-muted">
              asked {relativeTime(r.createdAt)}
              {#if r.requestedByName}by {r.requestedByName}{/if}
              {#if r.reason}— “{r.reason}”{/if}
            </div>
          </div>
          <Button
            variant="ghost"
            size="xs"
            disabled={deciding === r.agentModel}
            onclick={() => void decide(r.agentModel, r.agentName ?? r.agentModel, 'reject')}
          >
            Decline
          </Button>
          <Button
            size="xs"
            disabled={deciding === r.agentModel}
            onclick={() => void decide(r.agentModel, r.agentName ?? r.agentModel, 'approve')}
          >
            Approve
          </Button>
        </div>
      {/each}
    </div>
  {/if}

  <p class="mb-3 font-sans text-xs text-muted">
    Restrictive by default: a ticket can only be assigned to agents allowed here.
    {#if !canEdit}
      A personal assistant whose owner can read this board joins on its own; anything else waits for an editor.
    {/if}
  </p>
  {#if configFailed}
    <QueryError
      variant="compact"
      title="Could not load the agent policy"
      error={configQuery.error ?? fleetQuery.error}
      onRetry={() => {
        void configQuery.refetch()
        void fleetQuery.refetch()
      }}
    />
  {:else if configLoading || fleetLoading}
    <!-- The checkbox is seeded from the fetched config — showing it unchecked
         now would render a false policy that flips after load. -->
    <div>
      <Skeleton class="h-11 w-full" />
      <div class="mt-4 flex items-center justify-between gap-2">
        <Skeleton class="h-4 w-32" />
        <Skeleton class="h-8 w-16" />
      </div>
    </div>
  {:else if canEdit}
    {#if !allowAll}
      <Combobox {options} selected={agents} onChange={(next) => (agents = next)} multiple placeholder="Select agents" />
    {/if}
    <div class="mt-4 flex items-center justify-between gap-2">
      <Checkbox checked={allowAll} onChange={(checked) => (allowAll = checked)} label="Allow all agents" class="gap-2 font-sans text-sm text-fg" />
      <div class="flex items-center gap-2">
        {#if saved}<span class="font-mono text-[10px] uppercase tracking-[0.05em] text-success">Saved</span>{/if}
        <Button size="sm" onclick={() => void save()} disabled={busy}>
          Save
        </Button>
      </div>
    </div>
  {:else}
    <!-- Read-only for viewers: the policy renders as state, not controls, so
         nobody spends a Save on a 403. -->
    <div class="rounded-md border border-line px-3 py-2.5">
      {#if allowAll}
        <span class="font-sans text-sm text-fg">All agents are allowed on this board.</span>
      {:else if agents.length === 0}
        <span class="font-sans text-sm text-fg">No agents are allowed on this board.</span>
      {:else}
        <div class="font-sans text-sm text-fg">
          {agents.length}
          {agents.length === 1 ? 'agent is' : 'agents are'}
          allowed:
        </div>
        <ul class="mt-1 space-y-0.5">
          {#each agents as a (a)}
            <li class="font-mono text-[11px] text-muted">
              {a}
              {#if fleet?.agents.find((f) => f.id === a)?.label}
                <span class="ml-1.5 font-sans text-xs">{fleet.agents.find((f) => f.id === a)!.label}</span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

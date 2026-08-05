<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useAgents } from '@/lib/agents'
  import { setBoardAgents, useBoardAgents, type Board } from '@/lib/boards.svelte'

  // The Agents tab of BoardSettingsModal.svelte.
  let { board }: { board: Board } = $props()

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
  let allowAll = $state(false)
  let agents = $state<string[]>([])
  let busy = $state(false)
  let saved = $state(false)

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
</script>

<div>
  <p class="mb-3 font-sans text-xs text-muted">
    Restrictive by default: a ticket can only be assigned to agents allowed here.
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
        <Skeleton class="h-8 w-16" delay={0.12} />
      </div>
    </div>
  {:else}
    {#if !allowAll}
      <Combobox {options} selected={agents} onChange={(next) => (agents = next)} multiple placeholder="Select agents" />
    {/if}
    <div class="mt-4 flex items-center justify-between gap-2">
      <label class="flex cursor-pointer items-center gap-2 font-sans text-sm text-fg">
        <input
          type="checkbox"
          bind:checked={allowAll}
          class="accent-[color:var(--theme-accent)]"
        />
        Allow all agents
      </label>
      <div class="flex items-center gap-2">
        {#if saved}<span class="font-mono text-[10px] uppercase tracking-[0.05em] text-success">Saved</span>{/if}
        <Button size="sm" onclick={() => void save()} disabled={busy}>
          Save
        </Button>
      </div>
    </div>
  {/if}
</div>

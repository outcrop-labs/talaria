<script lang="ts">
  import { CalendarClock, Import, LayoutGrid, List, Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import CreateAgentModal from '@/components/fleet/CreateAgentModal.svelte'
  import FederateModal from '@/components/fleet/FederateModal.svelte'
  import FleetCronsModal from '@/components/fleet/FleetCronsModal.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { useFleet } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { useFleetContainers, useFleetDefs, type AgentDef } from '@/lib/fleet-defs'
  import { useSession } from '@/lib/session'
  import AgentListRow from './AgentListRow.svelte'
  import AgentTile from './AgentTile.svelte'

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const fleetQuery = useFleet()
  const defsQuery = useFleetDefs(() => isAdmin)
  const containersQuery = useFleetContainers(() => isAdmin)
  const byDept = $derived(new Map((containersQuery.data ?? []).map((c) => [c.department, c])))
  // The roster itself renders through QueryState below (a 500 must never read as
  // "no agents"). This copy is only for the create/duplicate modals' template
  // list, where an empty list is a harmless degradation.
  const defs = $derived(defsQuery.data?.defs ?? [])
  const t = $derived(fleetQuery.data?.totals)

  let view = $state<'grid' | 'list'>('grid')
  let creating = $state(false)
  let duplicateFrom = $state<AgentDef | null>(null)
  let schedulesOpen = $state(false)
  let federateOpen = $state(false)
</script>

<!-- The roster's placeholder — same tile grid the content lands in. -->
{#snippet rosterSkeleton()}
  <div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
    {#each [0, 1, 2, 3, 4, 5] as i (i)}
      <SkeletonCard delay={i * 0.1} />
    {/each}
  </div>
{/snippet}

{#snippet gridIcon()}<LayoutGrid size={14} />{/snippet}
{#snippet listIcon()}<List size={14} />{/snippet}

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto max-w-5xl space-y-6">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="font-sans text-2xl font-semibold tracking-tight text-fg">Agents</h1>
      {#if t}
        <span class="font-mono text-[11px] tracking-[0.05em] text-muted">{t.online}/{t.agents} online · {t.activeToday} active today</span>
      {:else if fleetQuery.isError}
        <!-- Missing totals with no explanation read as "nothing is running". -->
        <span class="font-mono text-[11px] tracking-[0.05em] text-danger" title={errorMessage(fleetQuery.error)}>
          Fleet status unavailable
        </span>
      {:else if fleetQuery.isLoading}
        <!-- Hold the slot so the toolbar doesn't jog when the totals land. -->
        <Skeleton class="h-3 w-40" />
      {/if}
      <div class="ml-auto flex items-center gap-2">
        <!-- Grid / list toggle — the §8 segmented tile pair. -->
        <Segmented
          options={[
            { id: 'grid', label: gridIcon, title: 'Grid' },
            { id: 'list', label: listIcon, title: 'List' },
          ]}
          value={view}
          onChange={(id) => (view = id)}
        />
        {#if isAdmin}
          <Button size="sm" class="w-9 px-0" onclick={() => (creating = true)} title="New agent" aria-label="New agent">
            <Plus size={16} />
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="w-9 px-0"
            onclick={() => (schedulesOpen = true)}
            title="Schedules: crons across the fleet"
            aria-label="Schedules"
          >
            <CalendarClock size={15} />
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="w-9 px-0"
            onclick={() => (federateOpen = true)}
            title="Federate outside agents into Talaria"
            aria-label="Federate agents"
          >
            <Import size={15} />
          </Button>
        {/if}
      </div>
    </div>

    <!-- "No agents yet" is a claim about the fleet, and for months a 500 from
         /api/fleet/defs made this surface state it — the owner reading that
         his fleet was empty while the box was down. It is now reachable ONLY
         from a 200 that really carried zero defs; the failure renders as a
         failure, and a disabled read (no manage access) says that instead. -->
    <QueryState
      query={defsQuery}
      errorTitle="Could not load the agent roster"
      errorVariant="compact"
      isEmpty={(d) => d.defs.length === 0}
      skeleton={rosterSkeleton}
    >
      {#snippet idle()}
        <Panel>
          <EmptyState
            title="Roster not available to you"
            hint="Agent definitions are served to accounts that manage the fleet — ask an admin for access."
          />
        </Panel>
      {/snippet}
      {#snippet empty()}
        <Panel>
          <EmptyState
            title="No agents yet"
            hint="Describe the first one and Muse designs it: identity, soul, and starter skills."
          >
            {#snippet action()}
              <Button size="sm" onclick={() => (creating = true)}>
                Design your first agent
              </Button>
            {/snippet}
          </EmptyState>
        </Panel>
      {/snippet}
      {#snippet children(data)}
        {@const endpoints = data.endpoints}
        {@const brainByAgent = new Map((data.brains ?? []).map((b) => [b.agent, b]))}
        <!-- Defs AND containers gate the grid together: tiles built from
             `containers: null` read every agent as stopped, then flip. -->
        {#if containersQuery.isLoading}
          {@render rosterSkeleton()}
        {:else}
          <div class="space-y-3">
            <!-- Docker unreachable ≠ every agent stopped. Without this the
                 dots all go red and the roster quietly libels the fleet. -->
            {#if containersQuery.isError}
              <div transition:slide={{ duration: 150 }}>
                <QueryError
                  variant="inline"
                  title={containersQuery.data === undefined ? 'Could not read container status' : 'Container status may be out of date'}
                  error={containersQuery.error}
                  onRetry={() => void containersQuery.refetch()}
                />
              </div>
            {/if}
            {#if view === 'grid'}
              <div class="grid grid-cols-2 gap-3 sm:grid-cols-3">
                {#each data.defs as d (d.id)}
                  <AgentTile def={d} containers={byDept.get(d.department) ?? null} {endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => (duplicateFrom = d)} />
                {/each}
              </div>
            {:else}
              <Panel class="p-0">
                <ul class="divide-y divide-line">
                  {#each data.defs as d (d.id)}
                    <AgentListRow def={d} containers={byDept.get(d.department) ?? null} {endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => (duplicateFrom = d)} />
                  {/each}
                </ul>
              </Panel>
            {/if}
          </div>
        {/if}
      {/snippet}
    </QueryState>

    {#if schedulesOpen}<FleetCronsModal onClose={() => (schedulesOpen = false)} />{/if}
    {#if federateOpen}<FederateModal onClose={() => (federateOpen = false)} />{/if}
    {#if creating}<CreateAgentModal open={creating} onClose={() => (creating = false)} templates={defs.filter((d) => d.enabled)} />{/if}
    {#if duplicateFrom}
      <CreateAgentModal open onClose={() => (duplicateFrom = null)} templates={defs} templateId={duplicateFrom.id} />
    {/if}
  </div>
</div>

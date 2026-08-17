<script lang="ts">
  import { Import, LayoutGrid, List, Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import CreateAgentModal from '@/components/fleet/CreateAgentModal.svelte'
  import RoleTemplatesTab from '@/components/fleet/RoleTemplatesTab.svelte'
  import FederateModal from '@/components/fleet/FederateModal.svelte'
  import FleetCronsTab from '@/components/fleet/FleetCronsTab.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { useFleet } from '@/lib/fleet'
  import { fly, slide, staggerIn } from '@/lib/motion'
  import { useFleetContainers, useFleetDefs, type AgentDef } from '@/lib/fleet-defs'
  import { useSession } from '@/lib/session'
  import { navigate, route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import AgentListRow from './AgentListRow.svelte'
  import AgentTile from './AgentTile.svelte'

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  // Agents is the home tab; the role library and the fleet's schedules are the
  // other two. Each was a takeover modal behind a toolbar icon — an affordance
  // for one action, not for a surface you come back to and read.
  //
  // THE URL IS THE TAB. `/agents`, `/agents/templates`, `/agents/schedules` —
  // so a tab can be linked to, bookmarked and reached with the back button,
  // and "send me the schedules screen" is a URL rather than a description of
  // where to click. An unrecognised segment falls back to the roster rather
  // than rendering nothing.
  const TABS = ['agents', 'templates', 'schedules'] as const
  type AgentsTab = (typeof TABS)[number]
  const tab = $derived(tabFromPath(route.pathname, '/agents', TABS, 'agents'))
  const goTab = (id: AgentsTab) => {
    // Two calls rather than one: the typed router wants the params object only
    // on the parameterised path, and the roster is the bare one.
    if (id === 'agents') void navigate('/agents')
    else void navigate('/agents/:tab', { params: { tab: id } })
  }
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
  let federateOpen = $state(false)

  // Defs AND containers gate the roster together: tiles built from
  // `containers: null` read every agent as stopped, then flip. (A disabled
  // containers query — non-admin — is never `isLoading`.)
  const rosterLoading = $derived(defsQuery.isLoading || containersQuery.isLoading)
  const gridClasses = 'grid grid-cols-2 gap-3 sm:grid-cols-3'
  const listClasses = 'divide-y divide-line overflow-hidden rounded-lg border border-line bg-panel'
  // The list container classes only apply while there are items (or their
  // skeletons) to frame — error/idle/empty render full-width, unframed.
  const rosterClasses = $derived(
    rosterLoading || (defsQuery.data?.defs.length ?? 0) > 0 ? (view === 'grid' ? gridClasses : listClasses) : undefined,
  )

  // Silhouette widths vary per index so the sketch doesn't look stamped.
  const nameW = ['w-24', 'w-32', 'w-20', 'w-28']
  const roleW = ['w-16', 'w-24', 'w-28', 'w-14']
</script>

<!-- One tile's silhouette (see AgentTile: avatar + name/role, then the
     controls row) — same Panel frame, so the real tile materializes over it. -->
{#snippet tileSkeleton(i: number)}
  <div aria-hidden="true" class="flex flex-col gap-3 rounded-lg border border-line bg-panel p-6">
    <div class="flex items-center gap-2.5">
      <Skeleton class="h-9 w-9 shrink-0 rounded-full" delay={i * 0.1} />
      <div class="min-w-0 flex-1 space-y-1.5">
        <Skeleton class={`h-3.5 rounded-full ${nameW[i % nameW.length]}`} delay={i * 0.1} />
        <Skeleton class={`h-2.5 rounded-full ${roleW[i % roleW.length]}`} delay={i * 0.1 + 0.1} />
      </div>
      <Skeleton class="h-2 w-2 shrink-0 rounded-full" delay={i * 0.1} />
    </div>
    <div class="flex justify-end gap-1">
      <Skeleton class="h-7 w-7 rounded-md" delay={i * 0.1 + 0.15} />
      <Skeleton class="h-7 w-7 rounded-md" delay={i * 0.1 + 0.2} />
    </div>
  </div>
{/snippet}

<!-- One list row's silhouette (see AgentListRow: dot + avatar + name/role + controls). -->
{#snippet rowSkeleton(i: number)}
  <div aria-hidden="true" class="flex items-center gap-3 px-4 py-3">
    <Skeleton class="h-2 w-2 shrink-0 rounded-full" delay={i * 0.1} />
    <Skeleton class="h-7 w-7 shrink-0 rounded-full" delay={i * 0.1} />
    <Skeleton class={`h-3.5 rounded-full ${nameW[i % nameW.length]}`} delay={i * 0.1} />
    <Skeleton class={`h-2.5 rounded-full ${roleW[(i + 1) % roleW.length]}`} delay={i * 0.1 + 0.1} />
    <span class="ml-auto flex shrink-0 items-center gap-1">
      <Skeleton class="h-7 w-7 rounded-md" delay={i * 0.1 + 0.15} />
      <Skeleton class="h-7 w-7 rounded-md" delay={i * 0.1 + 0.2} />
    </span>
  </div>
{/snippet}

{#snippet itemSkeleton(i: number)}
  {#if view === 'grid'}{@render tileSkeleton(i)}{:else}{@render rowSkeleton(i)}{/if}
{/snippet}

{#snippet gridIcon()}<LayoutGrid size={14} />{/snippet}
{#snippet listIcon()}<List size={14} />{/snippet}

<div class="h-full overflow-y-auto p-8">
  <!-- Page content entrance: toolbar row, then the roster region, rise in
       sequence (ANIMATIONS.md). The roster region is ONE child at this level;
       the tile cascade lives on the loaded branch's own stagger below. -->
  <div use:staggerIn class="mx-auto max-w-5xl space-y-6">
    <ViewHeader title="Agents">
      {#snippet status()}
        {#if t}
          <span>{t.online}/{t.agents} online · {t.activeToday} active today</span>
        {:else if fleetQuery.isError}
          <!-- Missing totals with no explanation read as "nothing is running". -->
          <span class="text-danger" title={errorMessage(fleetQuery.error)}>
            Fleet status unavailable
          </span>
        {:else if fleetQuery.isLoading}
          <!-- Hold the slot so the toolbar doesn't jog when the totals land. -->
          <Skeleton class="h-3 w-40" />
        {/if}
      {/snippet}
      {#snippet actions()}
        <!-- Roster controls, and only on the roster: a grid/list toggle means
             nothing next to a template library or a schedule list. -->
        {#if tab === 'agents'}
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
            onclick={() => (federateOpen = true)}
            title="Federate outside agents into Talaria"
            aria-label="Federate agents"
          >
            <Import size={15} />
          </Button>
        {/if}
        {/if}
      {/snippet}
    </ViewHeader>

    <Tabs
      items={[
        { id: 'agents', label: 'Agents' },
        { id: 'templates', label: 'Role templates' },
        { id: 'schedules', label: 'Schedules' },
      ]}
      value={tab}
      onChange={goTab}
    />

    <!-- Tab-pane grammar: rise in on switch, no exit (ANIMATIONS.md). -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} class="space-y-6">
        {#if tab === 'templates'}
          <RoleTemplatesTab />
        {:else if tab === 'schedules'}
          <FleetCronsTab />
        {:else}
    <!-- Docker unreachable ≠ every agent stopped. Without this the dots all
         go red and the roster quietly libels the fleet. -->
    {#if containersQuery.isError && defsQuery.data !== undefined}
      <div transition:slide={{ duration: 150 }}>
        <QueryError
          variant="inline"
          title={containersQuery.data === undefined ? 'Could not read container status' : 'Container status may be out of date'}
          error={containersQuery.error}
          onRetry={() => void containersQuery.refetch()}
        />
      </div>
    {/if}

    <!-- "No agents yet" is a claim about the fleet, and for months a 500 from
         /api/fleet/defs made this surface state it — the owner reading that
         his fleet was empty while the box was down. It is now reachable ONLY
         from a 200 that really carried zero defs; the failure renders as a
         failure, and a disabled read (no manage access) says that instead.

         Skeleton → content as one motion: view-shaped skeletons (tiles or
         rows) materialize into the roster — Materialize grid-stacks the
         branches and its content branch owns the cascade, replacing the old
         double skeleton swap (defs, then containers). QueryState stays for
         ERROR / IDLE / EMPTY; its own loading branch only covers the
         paused-fetch edge, with the same item silhouette. A view toggle
         remounts via {#key view} and keeps the tab-pane fly. -->
    {#key view}
      <div in:fly={{ y: 6, duration: 200 }}>
        <Materialize loading={rosterLoading} count={6} class={rosterClasses}>
          {#snippet skeleton(i)}{@render itemSkeleton(i)}{/snippet}
          <QueryState
            query={defsQuery}
            errorTitle="Could not load the agent roster"
            errorVariant="compact"
            isEmpty={(d) => d.defs.length === 0}
          >
            {#snippet skeleton()}{@render itemSkeleton(0)}{/snippet}
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
              {#each data.defs as d (d.id)}
                {#if view === 'grid'}
                  <AgentTile def={d} containers={byDept.get(d.department) ?? null} {endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => (duplicateFrom = d)} />
                {:else}
                  <AgentListRow def={d} containers={byDept.get(d.department) ?? null} {endpoints} brain={brainByAgent.get(d.model)} onDuplicate={() => (duplicateFrom = d)} />
                {/if}
              {/each}
            {/snippet}
          </QueryState>
        </Materialize>
      </div>
    {/key}
        {/if}
      </div>
    {/key}

    {#if federateOpen}<FederateModal onClose={() => (federateOpen = false)} />{/if}
    {#if creating}<CreateAgentModal open={creating} onClose={() => (creating = false)} templates={defs.filter((d) => d.enabled)} />{/if}
    {#if duplicateFrom}
      <CreateAgentModal open onClose={() => (duplicateFrom = null)} templates={defs} templateId={duplicateFrom.id} />
    {/if}
  </div>
</div>

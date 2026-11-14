<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsHeaderRow from '@/components/settings/SettingsHeaderRow.svelte'
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronRight, CircleAlert, Import, LayoutGrid, List, Repeat } from '@lucide/svelte'
  import { designHeadline, pendingDesign, useAgentDesign } from '@/lib/agent-design'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { searchParams } from 'sv-router'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { anyLoading } from '@/components/ui/query-state'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import CreateAgentModal from '@/components/fleet/CreateAgentModal.svelte'
  import HiringStrip from '@/components/fleet/HiringStrip.svelte'
  import RoleTemplatesTab from '@/components/fleet/RoleTemplatesTab.svelte'
  import FederateModal from '@/components/fleet/FederateModal.svelte'
  import SealConfigsModal from '@/components/fleet/SealConfigsModal.svelte'
  import FleetCronsTab from '@/components/fleet/FleetCronsTab.svelte'
  import { rosterCount, rosterLine } from '@/components/fleet/roster-count'
  import { useFleet } from '@/lib/fleet'
  import { errorMessage } from '@/lib/fetch-json'
  import { useSession } from '@/lib/session'
  import { fly, slide } from '@/lib/motion'
  import { containersOf, hiresToShow, rollFleet, rollRowsBySlug, useFleetContainers, useFleetDefs, useFleetHires, useFleetRoll, type AgentDef, type FleetRollStart } from '@/lib/fleet-defs'
  import { navigate, route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import { AGENTS_TABS } from '@/lib/settings-nav'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useModelNames } from '@/components/models/model-names.svelte'
  import AgentListRow, { LIST_ACTION, LIST_MODEL, LIST_STATUS } from './AgentListRow.svelte'
  import AgentTile from './AgentTile.svelte'

  const queryClient = useQueryClient()
  const session = useSession()
  // Federating an outside fleet is instance wiring, not agent management —
  // that route is admin-only, so the button is too.
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
  type AgentsTab = (typeof AGENTS_TABS)[number]['id']
  const tab = $derived(tabFromPath(route.pathname, '/agents', AGENTS_TABS.map((t) => t.id), 'agents'))
  const goTab = (id: AgentsTab) => {
    // Two calls rather than one: the typed router wants the params object only
    // on the parameterised path, and the roster is the bare one.
    if (id === 'agents') void navigate('/agents')
    else void navigate('/agents/:tab', { params: { tab: id } })
  }
  const fleetQuery = useFleet()
  // THE READS ARE NOT GATED CLIENT-SIDE ANY MORE. They used to be `isAdmin`,
  // from before an agent had managers: now the roster read answers with the
  // agents THIS person may see — the whole fleet for `agents.manage` and
  // admins, their own otherwise — and `canHire` says which it was
  // (docs/PERMISSIONS.md, "Agent managers"). Anyone who reaches this route
  // has the /agents view, which managing an agent grants.
  const defsQuery = useFleetDefs(() => true)
  const containersQuery = useFleetContainers(() => true)
  // Hires in flight, for the strip above the roster. Polls only while
  // something is actually hiring (the query owns that cadence).
  const canHire = $derived(defsQuery.data?.canHire ?? false)
  const hiresQuery = useFleetHires(() => canHire)

  // A hire this surface watched alive reaching a terminal state is the
  // landing event: the def row (and, after a boot, its container) already
  // exist server-side — the roster just hasn't heard. One invalidation per
  // hire, tracked in Sets because the hires window deliberately holds
  // finished rows for ten minutes. A hire that finished before this mount
  // fires nothing: the initial defs read already carried its tile.
  const seenAlive = new Set<string>()
  const actedOn = new Set<string>()
  $effect(() => {
    for (const h of hiresQuery.data?.hires ?? []) {
      if (h.state === 'queued' || h.state === 'running') {
        seenAlive.add(h.id)
        continue
      }
      if (actedOn.has(h.id) || !seenAlive.has(h.id)) continue
      actedOn.add(h.id)
      void queryClient.invalidateQueries({ queryKey: ['fleet-defs'] })
      void queryClient.invalidateQueries({ queryKey: ['fleet-containers'] })
    }
  })
  const byDept = $derived(new Map((containersQuery.data ?? []).map((c) => [c.department, c])))
  // The strip's rows: a failed hire's row (with its Retry) goes once it is
  // answered — a newer hire of the agent, or a container that has run
  // (`hiresToShow`). A container only `created` is still the hire's draft.
  const stripHires = $derived(
    hiresToShow(
      hiresQuery.data?.hires ?? [],
      new Set(
        (containersQuery.data ?? [])
          .filter((c) => c.managed && c.managed.state !== 'created')
          .map((c) => c.department),
      ),
    ),
  )

  // RESTARTING THE WHOLE FLEET (a roll: every agent on a fresh container).
  // The picture is admin-only (so is the route): one row per managed agent,
  // polled every five seconds while anything rolls. Every tile and row reads
  // its own line off it — the chip, the failure sentence, the disabled menu
  // item — and the strip above the roster reads the whole.
  const rollQuery = useFleetRoll(() => isAdmin)
  // By slug, not department (`rollRowsBySlug`): an agent left out of a shared
  // department reads its own idle row, not the runner's.
  const rollBySlug = $derived(rollRowsBySlug(rollQuery.data?.rolls))
  const anyRolling = $derived(rollQuery.data?.rolling ?? false)
  // The confirm step lives in the page, above the roster, not in a dialog:
  // what is about to happen is visible beside the agents it happens to.
  let rollConfirming = $state(false)
  let rollStopped = $state(false)
  let rollStarting = $state(false)
  // The batch THIS page started: its answer (what was queued, what was
  // skipped, or the refusal) stays up until dismissed or the next batch.
  let rollStart = $state<FleetRollStart | null>(null)
  const startRollAll = async () => {
    rollStarting = true
    try {
      rollStart = await rollFleet(rollStopped)
      rollConfirming = false
      await queryClient.invalidateQueries({ queryKey: ['fleet-roll'] })
    } finally {
      rollStarting = false
    }
  }
  // The batch's outcomes, from the live picture: done / failed counts and
  // the failure sentences, for the rows that belong to the batch this page
  // started (a manager's single roll keeps its own chip, not this strip).
  const batchRows = $derived.by(() => {
    const id = rollStart?.batch
    if (!id) return []
    return (rollQuery.data?.rolls ?? []).filter((r) => r.batch === id)
  })
  const batchRolling = $derived(batchRows.filter((r) => r.state === 'rolling').length)
  const batchDone = $derived(batchRows.filter((r) => r.state === 'done').length)
  const batchFailed = $derived(batchRows.filter((r) => r.state === 'failed'))
  const runningCount = $derived(
    (defsQuery.data?.defs ?? []).filter((d) => d.enabled && d.managed && (containersOf(d, byDept)?.managed?.state === 'running')).length,
  )
  // The roster itself renders through QueryState below (a 500 must never read as
  // "no agents"). This copy is only for the create/duplicate modals' template
  // list, where an empty list is a harmless degradation.
  const defs = $derived(defsQuery.data?.defs ?? [])
  // The roster's own line: the overview's count of the same agents, from the
  // same two reads, so the two pages say one thing. Who was active today is
  // the fleet's: when that read fails the line says so beside the count.
  const countLine = $derived.by(() => {
    const all = defsQuery.data?.defs
    if (!all || !containersQuery.data) return null
    const active = fleetQuery.data?.totals.activeToday
    return [...rosterLine(rosterCount(all, byDept)), ...(active === undefined ? [] : [`${active} active today`])].join(' · ')
  })

  const modelName = useModelNames()

  // The header keeps one filled button, New agent. Everything else that acts
  // on the whole roster is in the menu beside it.
  const headerMenu = (): ContextMenuEntry[] => [
    {
      label: anyRolling ? 'Restarting all agents' : 'Restart all agents',
      icon: [Repeat, { size: 14 }],
      // A second batch while one is in flight would only find every
      // department already rolling.
      disabled: anyRolling || rollConfirming,
      onSelect: () => (rollConfirming = true),
    },
    ...(canHire
      ? ([{ label: 'Federate outside agents', icon: [Import, { size: 14 }], onSelect: () => (federateOpen = true) }] as ContextMenuEntry[])
      : []),
  ]

  let view = $state<'grid' | 'list'>('grid')
  let creating = $state(false)
  let scheduling = $state(false)
  let duplicateFrom = $state<AgentDef | null>(null)
  let federateOpen = $state(false)
  let sealOpen = $state(false)
  // The design row and the New agent mark (TALA-11): the SERVER's design
  // (lib/agent-design.ts), so a reload, a second tab or another device shows
  // the same thing. Headline = first words of the purpose while generating
  // or failed, the designed agent's name once landed. Null when there is
  // none. Read only where the person may hire.
  const designQuery = useAgentDesign(() => canHire)
  const design = $derived(pendingDesign(designQuery.data?.design))
  const designRow = $derived(design ? { status: design.status, headline: designHeadline(design) } : null)
  // The bell's link back (`/agents?design=open`): open New agent on the
  // design, then drop the parameter so a reload does not reopen it.
  $effect(() => {
    if (searchParams.get('design') && canHire) {
      creating = true
      searchParams.delete('design')
    }
  })

  // Defs AND containers gate the roster together: tiles built from
  // `containers: null` read every agent as stopped, then flip. (A disabled
  // containers query — non-admin — is never `isLoading`.) Through
  // `anyLoading`, never `||`: with `||` the roster stayed on its skeletons
  // for good whenever containers answered before defs (anyLoading says why).
  const rosterLoading = $derived(anyLoading(defsQuery, containersQuery))
  // Three across only where the 960px column gives each card room for a
  // name and a role. A row is as tall as its tallest card, so footers line up
  // along it and a row of short cards has no empty band.
  const gridClasses = 'grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3'
  const listClasses = 'overflow-hidden rounded-lg border border-line bg-panel [&>*+*]:border-t [&>*+*]:border-line-subtle'
  // The grid frames its cards and their skeletons. The list's rows bring
  // their own card, so only its skeletons need one. Error, idle and empty
  // render full-width, unframed.
  const rosterClasses = $derived.by(() => {
    if (view === 'list') return rosterLoading ? listClasses : undefined
    return rosterLoading || (defsQuery.data?.defs.length ?? 0) > 0 ? gridClasses : undefined
  })

  // Silhouette widths vary per index so the sketch doesn't look stamped.
  const nameW = ['w-24', 'w-32', 'w-20', 'w-28']
  const roleW = ['w-40', 'w-48', 'w-36', 'w-44']
</script>

<!-- One tile's silhouette (see AgentTile: avatar + name/role, then the status
     line) in the same frame, so the real tile materializes over it. -->
{#snippet tileSkeleton(i: number)}
  <div aria-hidden="true" class="flex flex-col gap-4 rounded-lg border border-line bg-panel p-5">
    <div class="flex items-start gap-3">
      <Skeleton class="h-10 w-10 shrink-0 rounded-full" />
      <div class="min-w-0 flex-1 space-y-2 py-1">
        <Skeleton class={`h-3.5 rounded-full ${nameW[i % nameW.length]}`} />
        <Skeleton class={`h-3 rounded-full ${roleW[i % roleW.length]}`} />
      </div>
    </div>
    <div class="flex h-8 items-center gap-2">
      <!-- Fixed geometry, so nothing about it is uncertain: a flat dot, not a skeleton (UI-CONVENTIONS, Loading). -->
      <div class="h-1.5 w-1.5 shrink-0 rounded-full bg-line"></div>
      <Skeleton class="h-3 w-16 rounded-full" />
      <Skeleton class="ml-auto h-3 w-28 rounded-full" />
    </div>
  </div>
{/snippet}

<!-- One list row's silhouette (see AgentListRow: avatar + name/role + status + menu). -->
{#snippet rowSkeleton(i: number)}
  <div aria-hidden="true" class="flex min-h-[64px] items-center gap-4 px-5 py-3">
    <Skeleton class="h-8 w-8 shrink-0 rounded-full" />
    <div class="min-w-0 flex-1 space-y-2">
      <Skeleton class={`h-3.5 rounded-full ${nameW[i % nameW.length]}`} />
      <Skeleton class={`h-3 rounded-full ${roleW[(i + 1) % roleW.length]}`} />
    </div>
    <Skeleton class="h-3 w-16 rounded-full" />
    <Skeleton class="h-7 w-7 rounded-md" />
  </div>
{/snippet}

{#snippet itemSkeleton(i: number)}
  {#if view === 'grid'}{@render tileSkeleton(i)}{:else}{@render rowSkeleton(i)}{/if}
{/snippet}

{#snippet gridIcon()}<LayoutGrid size={16} class="my-0.5" />{/snippet}
{#snippet listIcon()}<List size={16} class="my-0.5" />{/snippet}

<!-- The roster region is ONE child of the page; the tile cascade lives on
     the loaded branch's own stagger below. -->
<SettingsPage
  title="Agents"
  blurb="The agents on your team, what they do, and whether they are running."
  tabs={AGENTS_TABS}
  {tab}
  onTab={goTab}
  fill={tab === 'templates'}
>
  {#snippet actions()}
    {#if tab === 'agents'}
      {#if isAdmin}
        <SettingsRowMenu label="More agent actions" items={headerMenu} />
      {/if}
      {#if canHire}
        <!-- The mark: a design generating or waiting for review keeps a dot
             on the entry point, wherever the row has scrolled to. -->
        <span class="relative inline-flex">
          <Button
            size="sm"
            onclick={() => (creating = true)}
            title={designRow ? (designRow.status === 'generating' ? 'A design is generating' : designRow.status === 'ready' ? 'A design is waiting for review' : 'The design failed') : undefined}
          >
            New agent
          </Button>
          {#if designRow}
            <span class="pointer-events-none absolute -right-0.5 -top-0.5 flex" data-testid="design-mark" data-status={designRow.status}>
              <StatusDot status={designRow.status === 'error' ? 'danger' : 'accent'} pulse={designRow.status === 'generating'} class="h-2 w-2" />
            </span>
          {/if}
        </span>
      {/if}
    {:else if tab === 'schedules'}
      <Button size="sm" onclick={() => (scheduling = true)}>
        New schedule
      </Button>
    {/if}
  {/snippet}

  <!-- Tab-pane grammar: rise in on switch, no exit (ANIMATIONS.md). -->
  {#key tab}
    <div in:fly={{ y: 6, duration: 200 }} class={tab === 'templates' ? 'h-full' : 'space-y-4'}>
      {#if tab === 'templates'}
        <RoleTemplatesTab />
      {:else if tab === 'schedules'}
        <FleetCronsTab bind:composing={scheduling} />
      {:else}
        <!-- The roster's own line: how much of it is up, and how to look at it.
             min-h-8 holds the line while the totals load. -->
        <div class="flex min-h-8 flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <span class="flex flex-wrap items-center gap-x-4 gap-y-1 font-sans text-[13px] leading-5 text-muted tabular-nums">
            {#if countLine !== null}
              {countLine}
            {:else if rosterLoading}
              <Skeleton class="h-3 w-40" />
            {/if}
            {#if fleetQuery.isError && fleetQuery.data === undefined}
              <span role="status" class="flex items-center gap-1.5" title={errorMessage(fleetQuery.error)}>
                <CircleAlert size={14} aria-hidden="true" class="shrink-0 text-danger" />
                Could not read today's activity
                <Button variant="link" size="sm" onclick={() => void fleetQuery.refetch()}>Retry</Button>
              </span>
            {/if}
          </span>
          <Segmented
            options={[
              { id: 'grid', label: gridIcon, title: 'Grid' },
              { id: 'list', label: listIcon, title: 'List' },
            ]}
            value={view}
            onChange={(id) => (view = id)}
          />
        </div>

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

        <!-- RESTART ALL: the confirm step lives in the page, above the roster,
             so what is about to happen is visible beside the agents it happens
             to. Nothing moves until the labeled button. -->
        {#if rollConfirming}
          <div transition:slide={{ duration: 150 }}>
            <Panel class="p-0">
              <div class="px-5 py-4">
                <p class="font-sans text-sm font-medium leading-5 text-fg">
                  Restart {runningCount === 1 ? 'the one running agent' : `all ${runningCount} running agents`}?
                </p>
                <p class="mt-0.5 font-sans text-[13px] leading-5 text-muted">
                  Each agent restarts on a fresh container and is offline for about a minute. Chats wait for it.
                </p>
              </div>
              <div class="flex flex-wrap items-center justify-between gap-3 border-t border-line-subtle px-5 py-3">
                <Checkbox checked={rollStopped} onChange={(v) => (rollStopped = v)} label="Also start agents that are stopped" />
                <div class="flex items-center gap-2">
                  <Button variant="ghost" size="sm" onclick={() => (rollConfirming = false)} disabled={rollStarting}>Cancel</Button>
                  <Button variant="outline" size="sm" onclick={() => void startRollAll()} disabled={rollStarting}>
                    {#if rollStarting}<WaitingMark site="fleet/agent-apply" size={12} />{/if}
                    Restart all agents
                  </Button>
                </div>
              </div>
            </Panel>
          </div>
        {/if}

        <!-- The batch this page started: progress while it runs, the tally and
             every failure sentence once it is done. A refusal (the route's
             error) lands in the same place. -->
        {#if rollStart}
          <div transition:slide={{ duration: 150 }}>
            <Panel class="space-y-1.5 px-5 py-3">
              <div class="flex items-center gap-3">
                {#if rollStart.error}
                  <CircleAlert size={16} class="shrink-0 text-danger" />
                  <span class="min-w-0 flex-1 font-sans text-sm leading-5 text-danger">Could not restart the agents: {rollStart.error}</span>
                {:else if batchRolling > 0}
                  <WaitingMark site="fleet/agent-apply" size={12} class="shrink-0 text-accent" />
                  <span class="min-w-0 flex-1 font-sans text-sm leading-5 text-fg tabular-nums">
                    Restarting agents: {batchDone + batchFailed.length} of {batchRows.length} done
                  </span>
                {:else if (rollStart.rolling?.length ?? 0) === 0}
                  <span class="min-w-0 flex-1 font-sans text-sm leading-5 text-muted">No agents to restart.</span>
                {:else}
                  <span class="min-w-0 flex-1 font-sans text-sm leading-5 text-fg tabular-nums">
                    Restarted {batchDone} of {batchRows.length || rollStart.rolling?.length || 0}{batchFailed.length > 0 ? `, ${batchFailed.length} failed` : ''}
                  </span>
                {/if}
                <CloseButton label="Dismiss" onClick={() => (rollStart = null)} class="-mr-1.5" />
              </div>
              {#if (rollStart.skipped?.length ?? 0) > 0}
                <p class="font-sans text-[13px] leading-5 text-muted">
                  Skipped: {rollStart.skipped?.map((s) => `${s.slug} (${s.why})`).join(', ')}
                </p>
              {/if}
              {#if batchFailed.length > 0}
                <ul class="space-y-0.5">
                  {#each batchFailed as f (f.slug)}
                    <li class="font-sans text-[13px] leading-5 text-danger">{f.displayName}: {f.detail ?? 'the restart failed'}</li>
                  {/each}
                </ul>
              {/if}
            </Panel>
          </div>
        {/if}

        <!-- Hires land here, above the tiles they become: a row per agent being
             created (the run's own phase sentence), a row per recent failure.
             Nothing renders once the work is done — the tile materializing over
             the strip IS the completion event. -->
        <HiringStrip hires={stripHires} />

        <!-- TALA-11: an agent DESIGN running in the background (or finished but
             unclaimed) — the same shelf the hires use, one slim status row. It
             is a pointer into the modal, not a surface of its own: click to
             re-enter the work. Renders nothing when there is no design. -->
        {#if designRow}
          <div transition:slide={{ duration: 150 }}>
            <Panel class="overflow-hidden p-0">
              <button
                type="button"
                class="flex w-full items-center gap-3 px-5 py-3.5 text-left transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
                onclick={() => {
                  creating = true
                }}
              >
                {#if designRow.status === 'generating'}
                  <WaitingMark site="fleet/agent-design" size={12} class="shrink-0 text-accent" />
                  <span class="min-w-0 flex-1 truncate font-sans text-sm leading-5 text-muted">Designing {designRow.headline}</span>
                {:else if designRow.status === 'error'}
                  <CircleAlert size={16} class="shrink-0 text-danger" />
                  <span class="min-w-0 flex-1 truncate font-sans text-sm leading-5 text-danger">The design failed: {designRow.headline}</span>
                {:else}
                  <StatusDot status="accent" class="shrink-0" />
                  <span class="min-w-0 flex-1 truncate font-sans text-sm leading-5 text-fg">{designRow.headline} is designed and waiting for review</span>
                {/if}
                <ChevronRight size={16} class="shrink-0 text-muted" />
              </button>
            </Panel>
          </div>
        {/if}

        <!-- "No agents yet" is a claim about the fleet, and for months a 500 from
             /api/fleet/defs made this surface state it — the owner reading that
             his fleet was empty while the box was down. It is now reachable ONLY
             from a 200 that really carried zero defs; the failure renders as a
             failure, and a disabled read (no manage access) says that instead.

             Skeleton → content as one motion: view-shaped skeletons (tiles or
             rows) materialize into the roster — Materialize grid-stacks the
             branches and its content branch owns the cascade. QueryState stays
             for ERROR / IDLE / EMPTY; its own loading branch only covers the
             paused-fetch edge, with the same item silhouette. A view switch
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
                idle={null}
              >
                {#snippet skeleton()}{@render itemSkeleton(0)}{/snippet}
                {#snippet empty()}
                  <!-- TWO DIFFERENT EMPTIES, and the server says which. Somebody
                       who can hire has no agents yet; somebody who cannot has
                       none of their own, and "design your first agent" under a
                       button they will be 403'd by is the wrong sentence. -->
                  {#if canHire}
                    <Panel>
                      <EmptyState title="No agents yet" hint="Describe the first one and Muse designs it for you.">
                        {#snippet action()}
                          <Button variant="outline" size="sm" onclick={() => (creating = true)}>Design your first agent</Button>
                        {/snippet}
                      </EmptyState>
                    </Panel>
                  {:else}
                    <!-- p-0: the zero state brings its own padding and its vignette fills this
                         box, so panel padding would be a band it cannot reach. -->
                    <Panel class="p-0">
                      <EmptyState
                        title="No agents are yours to manage"
                        hint="An agent shows here once you are one of its managers. Ask an admin to add you."
                      />
                    </Panel>
                  {/if}
                {/snippet}
                {#snippet children(data)}
                  {@const endpoints = data.endpoints}
                  {@const brainByAgent = new Map((data.brains ?? []).map((b) => [b.agent, b]))}
                  {#if view === 'grid'}
                    {#each data.defs as d (d.id)}
                      <AgentTile def={d} containers={containersOf(d, byDept)} {endpoints} brain={brainByAgent.get(d.model)} roll={rollBySlug.get(d.slug)} modelName={modelName(d.latest?.config?.main?.model)} onDuplicate={() => (duplicateFrom = d)} />
                    {/each}
                  {:else}
                    <SettingsGroup>
                      <SettingsHeaderRow label="Agent" menu>
                        <span class={LIST_MODEL}>Model</span>
                        <span class={LIST_STATUS}>Status</span>
                        <span class={LIST_ACTION}></span>
                      </SettingsHeaderRow>
                      {#each data.defs as d (d.id)}
                        <AgentListRow def={d} containers={containersOf(d, byDept)} {endpoints} brain={brainByAgent.get(d.model)} roll={rollBySlug.get(d.slug)} modelName={modelName(d.latest?.config?.main?.model)} onDuplicate={() => (duplicateFrom = d)} />
                      {/each}
                    </SettingsGroup>
                  {/if}
                {/snippet}
              </QueryState>
            </Materialize>
          </div>
        {/key}
      {/if}
    </div>
  {/key}

  {#if federateOpen}
    <FederateModal
      onClose={() => (federateOpen = false)}
      onSealStored={() => {
        federateOpen = false
        sealOpen = true
      }}
    />
  {/if}
  {#if sealOpen}<SealConfigsModal onClose={() => (sealOpen = false)} />{/if}
  {#if creating}<CreateAgentModal open={creating} onClose={() => (creating = false)} templates={defs.filter((d) => d.enabled)} />{/if}
  {#if duplicateFrom}
    <CreateAgentModal open onClose={() => (duplicateFrom = null)} templates={defs} templateId={duplicateFrom.id} />
  {/if}
</SettingsPage>

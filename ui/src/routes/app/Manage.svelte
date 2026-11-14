<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import ActivityPanel from '@/components/manage/ActivityPanel.svelte'
  import AgentsTile from '@/components/manage/AgentsTile.svelte'
  import AppsCount from '@/components/manage/AppsCount.svelte'
  import AttentionPanel from '@/components/manage/AttentionPanel.svelte'
  import FleetControls from '@/components/manage/FleetControls.svelte'
  import HermesChannelPanel from '@/components/manage/HermesChannelPanel.svelte'
  import McpTile from '@/components/manage/McpTile.svelte'
  import ModelsTile from '@/components/manage/ModelsTile.svelte'
  import SpendTile from '@/components/manage/SpendTile.svelte'
  import SkillsCount from '@/components/manage/SkillsCount.svelte'
  import TeamsCount from '@/components/manage/TeamsCount.svelte'
  import TemplatesCount from '@/components/manage/TemplatesCount.svelte'
  import WorkbenchHarnessPanel from '@/components/manage/WorkbenchHarnessPanel.svelte'
  import { factLine } from '@/components/observability/grammar'
  import { useEnabledApps } from '@/lib/apps'
  import { manageArea } from '@/lib/manage-area'
  import { NAV } from '@/lib/nav'
  import { useAppGrants, useDeniedViews, useHasPerm, useSession } from '@/lib/session'
  import { navigate, p, route } from '@/router'

  // THE MANAGE OVERVIEW — where the dock's gear lands. The first screen is
  // the status: what needs someone, four health numbers, what exists as one
  // line of counts, and what changed lately. Every number is a doorway into
  // the page that owns it. The instance-wide controls are actions, not
  // status, so they wait behind one closed row at the foot.
  //
  // WHAT A VIEWER SEES IS WHAT THEY CAN OPEN. A number renders only for a view
  // this person has been granted (the same `manageArea` the sidebar lists),
  // and only when its read is one their role may make: the provider list and
  // the controls are admin reads on the server, MCP needs agents.manage. A
  // number that is not rendered never sends its request.
  const session = useSession()
  const denied = useDeniedViews()
  const appGrants = useAppGrants()
  const canManageAgents = useHasPerm('agents.manage')
  const appsQuery = useEnabledApps()

  const isAdmin = $derived(session.data?.role === 'admin')
  const area = $derived(manageArea(NAV, appsQuery.data ?? [], { isAdmin, denied: denied.current, appGrants: appGrants.current }, route.pathname))
  const has = (to: string) => area.items.some((i) => i.to === to)

  const health = $derived({
    agents: has('/agents'),
    spend: has('/observability'),
    models: has('/models') && isAdmin,
    mcp: has('/mcp') && canManageAgents.current,
  })
  const inventory = $derived({
    apps: has('/apps'),
    teams: has('/teams'),
    templates: has('/templates'),
    studio: has('/studio'),
  })
  const some = (group: Record<string, boolean>) => Object.values(group).some(Boolean)
  // The alerts list lives under Observability, and much of it — the retrieval
  // plane, model routing, the outbox — is fixed from places only an admin or
  // an Observability grant reaches. A member granted only Agents would get a
  // list of things they cannot act on.
  const showAttention = $derived(isAdmin || has('/observability'))

  // The closed Fleet controls row and the gap above it.
  const FLEET_ROW = 82
  let imageNote = $state<string>()
  let toolsNote = $state<string>()
  const fleetNote = $derived.by(() => {
    const notes = [imageNote, toolsNote].filter((n) => n !== undefined).join(', ')
    return notes ? notes.charAt(0).toUpperCase() + notes.slice(1) : 'Restart agents, check tool servers, pin versions'
  })
</script>

<SettingsPage title="Overview" blurb="What needs attention across your workspace, and where to fix it.">

    {#if session.data && area.items.length === 0}
      <EmptyState
        icon="⚙"
        title="Nothing to manage here"
        hint="Managing agents, models and the rest is granted per person. Ask an admin if you need it."
      >
        {#snippet action()}
          <Button size="sm" variant="outline" onclick={() => void navigate('/home')}>Back to your inbox</Button>
        {/snippet}
      </EmptyState>
    {:else}
      {#if showAttention}<AttentionPanel />{/if}

      {#if some(health) || some(inventory)}
        <div class="space-y-3">
          {#if some(health)}
            <StatStrip>
              {#if health.agents}<AgentsTile />{/if}
              {#if health.spend}<SpendTile />{/if}
              {#if health.models}<ModelsTile />{/if}
              {#if health.mcp}<McpTile />{/if}
            </StatStrip>
          {/if}
          {#if some(inventory)}
            <p class={factLine}>
              {#if inventory.apps}<AppsCount />{/if}
              {#if inventory.teams}<TeamsCount />{/if}
              {#if inventory.templates}<TemplatesCount />{/if}
              {#if inventory.studio}<SkillsCount />{/if}
            </p>
          {/if}
        </div>
      {/if}

      <ActivityPanel
        allHref={has('/observability') ? p('/observability/:tab', { params: { tab: 'audit' } }) : undefined}
        below={isAdmin ? FLEET_ROW : 0}
      />

      {#if isAdmin}
        <!-- keepMounted: a rollout or a test in flight shows on the closed row. -->
        <Disclosure variant="card" keepMounted>
          {#snippet title()}Fleet controls<span class="ml-2 font-normal text-muted">{fleetNote}</span>{/snippet}
          <!-- The last row brings its own padding to the foot of the card. -->
          <div class="-mb-4 space-y-2">
            <FleetControls />
            <HermesChannelPanel bind:note={imageNote} />
            <WorkbenchHarnessPanel bind:note={toolsNote} />
          </div>
        </Disclosure>
      {/if}
    {/if}
</SettingsPage>

<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import ActivityPanel from '@/components/manage/ActivityPanel.svelte'
  import AgentsTile from '@/components/manage/AgentsTile.svelte'
  import AppsTile from '@/components/manage/AppsTile.svelte'
  import AttentionPanel from '@/components/manage/AttentionPanel.svelte'
  import FleetControls from '@/components/manage/FleetControls.svelte'
  import McpTile from '@/components/manage/McpTile.svelte'
  import ModelsTile from '@/components/manage/ModelsTile.svelte'
  import SpendTile from '@/components/manage/SpendTile.svelte'
  import StudioTile from '@/components/manage/StudioTile.svelte'
  import TeamsTile from '@/components/manage/TeamsTile.svelte'
  import TemplatesTile from '@/components/manage/TemplatesTile.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import { manageArea } from '@/lib/manage-area'
  import { staggerIn } from '@/lib/motion'
  import { NAV } from '@/lib/nav'
  import { useDeniedViews, useHasPerm, useSession } from '@/lib/session'
  import { navigate, route } from '@/router'

  // THE MANAGE OVERVIEW — where the dock's gear lands. The control plane at a
  // glance: what needs someone, one number per area with its trouble beside
  // it, the instance-wide controls, and what changed lately. Every number is
  // a doorway into the page that owns it, and each area's page stays one
  // click away in the sidebar docked beside this one.
  //
  // WHAT A VIEWER SEES IS WHAT THEY CAN OPEN. A tile renders only for a view
  // this person has been granted (the same `manageArea` the sidebar lists),
  // and only when its read is one their role may make: the provider list and
  // the controls are admin reads on the server, MCP needs agents.manage. A
  // tile that is not rendered never sends its request.
  const session = useSession()
  const denied = useDeniedViews()
  const canManageAgents = useHasPerm('agents.manage')
  const appsQuery = useEnabledApps()

  const isAdmin = $derived(session.data?.role === 'admin')
  const area = $derived(manageArea(NAV, appsQuery.data ?? [], { isAdmin, denied: denied.current }, route.pathname))
  const has = (to: string) => area.items.some((i) => i.to === to)

  const tiles = $derived({
    agents: has('/agents'),
    spend: has('/observability'),
    models: has('/models') && isAdmin,
    mcp: has('/mcp') && canManageAgents.current,
    apps: has('/apps'),
    teams: has('/teams'),
    templates: has('/templates'),
    studio: has('/studio'),
  })
  const anyTile = $derived(Object.values(tiles).some(Boolean))
  // The alerts list lives under Observability, and much of it — the retrieval
  // plane, model routing, the outbox — is fixed from places only an admin or
  // an Observability grant reaches. A member granted only Agents would get a
  // list of things they cannot act on.
  const showAttention = $derived(isAdmin || has('/observability'))
</script>

<PageSurface>
  <div use:staggerIn class="space-y-6">
    <ViewHeader title="Manage" blurb="The control plane at a glance. Every number opens the page that owns it." />

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

      {#if anyTile}
        <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {#if tiles.agents}<AgentsTile />{/if}
          {#if tiles.spend}<SpendTile />{/if}
          {#if tiles.models}<ModelsTile />{/if}
          {#if tiles.mcp}<McpTile />{/if}
          {#if tiles.apps}<AppsTile />{/if}
          {#if tiles.teams}<TeamsTile />{/if}
          {#if tiles.templates}<TemplatesTile />{/if}
          {#if tiles.studio}<StudioTile />{/if}
        </div>
      {/if}

      <div class="grid items-start gap-4 lg:grid-cols-2">
        {#if isAdmin}<FleetControls />{/if}
        <ActivityPanel />
      </div>
    {/if}
  </div>
</PageSurface>

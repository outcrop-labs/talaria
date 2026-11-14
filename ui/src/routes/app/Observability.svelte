<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { tabFromPath } from '@/lib/route-tabs'
  import { navigate, route } from '@/router'
  import { fly } from '@/lib/motion'
  import ComputePanel from '@/components/observability/ComputePanel.svelte'
  import HostPanel from '@/components/observability/HostPanel.svelte'
  import CostPanel from '@/components/observability/CostPanel.svelte'
  import AuditPanel from '@/components/observability/AuditPanel.svelte'
  import AlertsPanel from '@/components/observability/AlertsPanel.svelte'
  import McpCallsPanel from '@/components/observability/McpCallsPanel.svelte'
  import OverviewPanel from './observability/OverviewPanel.svelte'
  import { OBS_TABS, type ObsTab } from './observability/observability'

  // /observability/cost deep-links a detail view; the root is Overview.
  // THE URL IS THE TAB — /observability and /observability/<tab>.
  const tab = $derived(tabFromPath(route.pathname, '/observability', OBS_TABS.map((t) => t.id), 'overview'))
  const setTab = (t: ObsTab) => {
    if (t === 'overview') void navigate('/observability')
    else void navigate('/observability/:tab', { params: { tab: t } })
  }
</script>

<SettingsPage
  title="Observability"
  blurb="Health, cost and activity across the workspace."
  tabs={OBS_TABS}
  {tab}
  onTab={setTab}
  staggerItems="[data-obs-sections] > *"
>
    <!-- Tab-pane grammar: rise in on switch, no exit, no AutoHeight. On mount
         Overview's blocks (the [data-obs-sections] hook) rise 30ms apart while
         the frame fades. The detail panels stay one block each. -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} data-stagger-items="[data-obs-sections] > *">
        {#if tab === 'overview'}<OverviewPanel />{/if}
        {#if tab === 'compute'}<ComputePanel />{/if}
        {#if tab === 'host'}<HostPanel />{/if}
        {#if tab === 'cost'}<CostPanel />{/if}
        {#if tab === 'audit'}<AuditPanel />{/if}
        {#if tab === 'mcp'}<McpCallsPanel />{/if}
        {#if tab === 'alerts'}<AlertsPanel />{/if}
      </div>
    {/key}
</SettingsPage>

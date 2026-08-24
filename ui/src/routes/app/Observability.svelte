<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import { tabFromPath } from '@/lib/route-tabs'
  import { navigate, route } from '@/router'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import ComputePanel from '@/components/observability/ComputePanel.svelte'
  import CostPanel from '@/components/observability/CostPanel.svelte'
  import AuditPanel from '@/components/observability/AuditPanel.svelte'
  import AlertsPanel from '@/components/observability/AlertsPanel.svelte'
  import OverviewPanel from './observability/OverviewPanel.svelte'
  import { OBS_TABS, type ObsTab } from './observability/observability'

  // /observability/cost deep-links a detail view; the root is Overview.
  // THE URL IS THE TAB — /observability and /observability/<tab>.
  const tab = $derived(tabFromPath(route.pathname, '/observability', OBS_TABS.map((t) => t.id), 'overview'))
  const setTab = (t: ObsTab) => {
    if (t === 'overview') void navigate('/observability')
    else void navigate('/observability/:tab', { params: { tab: t } })
  }

  // The view's title lives in the top strip (lib/view-title) — the body opens
  // straight onto its tabs.
  claimViewTitle('Observability')
</script>

<PageSurface>
  <!-- Page content entrance: tab strip → panel region rise in sequence
       (ANIMATIONS.md). The keyed pane keeps its own fly on tab switch and
       stays unstaggered inside — one level only. -->
  <div use:staggerIn class="space-y-6">
    <Tabs items={OBS_TABS} value={tab} onChange={setTab} />
    <!-- Tab-pane grammar: rise in on switch, no exit; no AutoHeight. On MOUNT
         the pane is a section whose items cascade: Overview's three blocks
         (alerts strip, stat-tile grid, audit trail — the [data-obs-sections]
         hook) rise 30ms apart while the frame fades. The detail panels live in
         components/observability and stay one block — dense readouts, and not
         this file's cascade to own. -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} data-stagger-items="[data-obs-sections] > *">
        {#if tab === 'overview'}<OverviewPanel onOpen={setTab} />{/if}
        {#if tab === 'compute'}<ComputePanel />{/if}
        {#if tab === 'cost'}<CostPanel />{/if}
        {#if tab === 'audit'}<AuditPanel />{/if}
        {#if tab === 'alerts'}<AlertsPanel />{/if}
      </div>
    {/key}
  </div>
</PageSurface>

<script lang="ts">
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import Tabs from '@/components/ui/Tabs.svelte'
  import ComputePanel from '@/components/observability/ComputePanel.svelte'
  import CostPanel from '@/components/observability/CostPanel.svelte'
  import AuditPanel from '@/components/observability/AuditPanel.svelte'
  import AlertsPanel from '@/components/observability/AlertsPanel.svelte'
  import OverviewPanel from './observability/OverviewPanel.svelte'
  import { OBS_TABS, type ObsTab } from './observability/observability'

  // /observability?tab=cost deep-links a detail view; the root is Overview.
  const tab = $derived.by((): ObsTab => {
    const t = OBS_TABS.find((v) => v.id === searchParams.get('tab'))
    return t && t.id !== 'overview' ? t.id : 'overview'
  })
  const setTab = (t: ObsTab) => void navigate('/observability', { search: t === 'overview' ? {} : { tab: t } })
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto max-w-5xl space-y-6">
    <h1 class="font-sans text-2xl font-semibold tracking-tight text-fg">Observability</h1>
    <Tabs items={OBS_TABS} value={tab} onChange={setTab} />
    {#if tab === 'overview'}<OverviewPanel onOpen={setTab} />{/if}
    {#if tab === 'compute'}<ComputePanel />{/if}
    {#if tab === 'cost'}<CostPanel />{/if}
    {#if tab === 'audit'}<AuditPanel />{/if}
    {#if tab === 'alerts'}<AlertsPanel />{/if}
  </div>
</div>

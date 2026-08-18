<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { useAdminSettings } from './admin'

  const qc = useQueryClient()
  const query = useAdminSettings()
  const data = $derived(query.data)
  let days = $state('')
  const value = $derived(days !== '' ? days : String(data?.auditRetentionDays ?? ''))
  const save = async () => {
    const n = Number(value)
    if (!Number.isFinite(n)) return
    await fetch('/api/admin/settings', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ auditRetentionDays: n }),
    })
    days = ''
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }
</script>

<Panel>
  <SectionHeader class="mb-2" title="Settings" />
  {#if query.isPending}
    <!-- Hold the retention row so the input never renders blank, then fills. -->
    <div class="flex items-center gap-3">
      <div class="min-w-0 flex-1 space-y-1.5">
        <Skeleton class="h-3 w-28 rounded-full" />
        <Skeleton class="h-2.5 w-64 rounded-full" />
      </div>
      <Skeleton class="h-8 w-24 shrink-0" />
      <Skeleton class="h-7 w-16 shrink-0" />
    </div>
  {:else if !data}
    <!-- An empty retention box reads as "0 = keep forever". Saving from there
         would silently change the org's audit policy. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load your settings"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="flex items-center gap-3">
      <div class="min-w-0 flex-1">
        <div class="text-sm text-fg">Audit retention</div>
        <div class="text-xs text-muted">How many days to keep the audit log. 0 = keep forever.</div>
      </div>
      <Input
        size="sm"
        type="number"
        {value}
        oninput={(e) => (days = e.currentTarget.value)}
        onkeydown={submitOnEnter(() => days !== '' && Number(days) !== data?.auditRetentionDays && void save())}
        class="w-24 shrink-0"
      />
      <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">days</span>
      <Button size="sm" class="shrink-0" onclick={() => void save()} disabled={days === '' || Number(days) === data?.auditRetentionDays}>
        Save
      </Button>
    </div>
  {/if}
</Panel>

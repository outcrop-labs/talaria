<script lang="ts">
  import { alert, Button, Chip, confirm, Panel, useAppInvalidate, useAppQuery, useMe } from '@talaria/sdk'
  import { api, APP } from './contacts'

  // ── Manage surface ─────────────────────────────────────────────────────────
  const me = useMe()
  const stats = useAppQuery<{ total: number; byStage: Record<string, number> }>(APP, 'stats')
  const invalidate = useAppInvalidate(APP)

  const wipe = async () => {
    if (!(await confirm({ title: 'Wipe ALL contacts data?', message: 'Every contact and setting this app stored is deleted. This cannot be undone.', confirmLabel: 'Wipe everything', danger: true }))) return
    try {
      await api.post('wipe')
      await invalidate()
      await stats.refetch()
    } catch (e) {
      void alert({ title: 'Could not wipe', message: (e as Error).message })
    }
  }
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto w-full max-w-2xl space-y-6">
    <h1 class="font-sans text-lg font-semibold text-fg">Contacts data</h1>
    <!-- §8 stat block: big sans numeral + 10px mono uppercase label. -->
    <Panel>
      <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Contacts</div>
      <div class="mt-1.5 font-sans text-2xl font-semibold text-fg">{stats.data?.total ?? 0}</div>
      <div class="mt-3 flex flex-wrap gap-2">
        {#each Object.entries(stats.data?.byStage ?? {}) as [stage, n] (stage)}
          <Chip class="capitalize">{stage}: {n}</Chip>
        {/each}
      </div>
    </Panel>
    {#if me.data?.role === 'admin'}
      <!-- §8 danger zone: orange hairline panel, orange mono header, right
           meta, muted body — destructive button stays an orange outline. -->
      <Panel class="border-danger/40">
        <div class="mb-3 flex items-baseline justify-between gap-2">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Danger zone</span>
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Irreversible</span>
        </div>
        <p class="mb-3 text-xs text-muted">Remove everything this app has stored.</p>
        <Button variant="danger" size="sm" onclick={() => void wipe()}>Wipe all data</Button>
      </Panel>
    {/if}
  </div>
</div>

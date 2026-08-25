<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'

  interface OutreachData {
    config: { enabled: boolean; intervalMinutes: number; dailyDmCap: number }
    agents: Array<{ model: string; displayName: string; proactive: boolean; personal: boolean }>
    events: Array<{ agentModel: string; kind: string; note: string | null; createdAt: string }>
  }

  // Proactive outreach (#59): opt-in periodic check-ins where agents surface
  // things through their own governed tools, plus agent-initiated DM caps.
  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['admin-outreach'],
    queryFn: (): Promise<OutreachData> => getJson<OutreachData>('/api/admin/outreach'),
    refetchInterval: 30_000,
  }))
  const data = $derived(query.data)
  const cfg = $derived(data?.config)
  const agents = $derived(data?.agents ?? [])
  const save = async (patch: Partial<OutreachData['config']>, proactiveAgents?: string[]) => {
    if (!cfg) return
    const body = {
      ...cfg,
      ...patch,
      proactiveAgents: proactiveAgents ?? agents.filter((a) => a.proactive).map((a) => a.model),
    }
    await fetch('/api/admin/outreach', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    await qc.invalidateQueries({ queryKey: ['admin-outreach'] })
  }
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="Proactive outreach"
    info="Opted-in agents get a periodic check-in: a look at their stale or blocked work, and the chance to act through their normal tools (a ticket comment, a channel post, or a direct message to your inbox). Everything stays attributed and board-policy-gated; DMs are capped per person per day. Off by default."
  />
  {#if query.isPending}
    <!-- Controls, agent chips, and the recent list all seed from the query —
         hold their footprint so the toggle never flashes unchecked. -->
    <div class="flex flex-wrap items-center gap-4">
      <Skeleton class="h-4 w-44 rounded-full" />
      <Skeleton class="h-8 w-32" />
      <Skeleton class="h-8 w-28" />
    </div>
    <div class="mt-3">
      <Skeleton class="mb-2 h-2.5 w-28 rounded-full" />
      <div class="flex flex-wrap gap-x-4 gap-y-1.5">
        {#each Array.from({ length: 4 }, (_, i) => i) as i (i)}
          <Skeleton class="h-4 w-28 rounded-full" />
        {/each}
      </div>
    </div>
    <div class="mt-4">
      <SkeletonRows rows={3} />
    </div>
  {:else if !data}
    <!-- Same shape as the guard panel: an unchecked toggle and an empty agent
         list are this component's defaults, not the org's actual policy. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load outreach settings"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="flex flex-wrap items-center gap-4">
      <Checkbox
        class="gap-2"
        checked={cfg?.enabled ?? false}
        onChange={(checked) => void save({ enabled: checked })}
        label="Enable periodic check-ins"
      />
      <label class="flex items-center gap-2 text-xs text-muted" title="Minimum minutes between one agent's check-ins">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Every</span>
        <Input
          size="sm" type="number" min={15} max={1440} class="w-20"
          value={cfg?.intervalMinutes ?? 240}
          disabled={!cfg?.enabled}
          oninput={(e) => void save({ intervalMinutes: Math.max(15, Math.min(1440, Number(e.currentTarget.value) || 240)) })}
        />
        <span>min</span>
      </label>
      <label class="flex items-center gap-2 text-xs text-muted" title="Agent-initiated direct messages allowed per person per day">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">DM cap</span>
        <Input
          size="sm" type="number" min={1} max={20} class="w-16"
          value={cfg?.dailyDmCap ?? 3}
          oninput={(e) => void save({ dailyDmCap: Math.max(1, Math.min(20, Number(e.currentTarget.value) || 3)) })}
        />
        <span>/day</span>
      </label>
    </div>
    {#if agents.length > 0}
      <div class="mt-3">
        <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Proactive agents</div>
        <div class="flex flex-wrap gap-x-4 gap-y-1.5">
          {#each agents as a (a.model)}
            {#snippet agentLabel()}
              {a.displayName}
              {#if a.personal}<span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">personal</span>{/if}
            {/snippet}
            <Checkbox
              class="gap-2 text-sm text-fg"
              checked={a.proactive}
              onChange={(checked) => {
                const next = agents.filter((x) => (x.model === a.model ? checked : x.proactive)).map((x) => x.model)
                void save({}, next)
              }}
              label={agentLabel}
            />
          {/each}
        </div>
      </div>
    {/if}
    {#if data?.events && data.events.length > 0}
      <div class="mt-4">
        <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Recent outreach</div>
        <div class="max-h-48 divide-y divide-line-subtle overflow-y-auto rounded-md border border-line">
          {#each data.events as ev, i (i)}
            <div class="flex items-start gap-2 px-2 py-1.5 text-xs transition-colors dither-fill">
              <span class="shrink-0 rounded px-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{ev.kind}</span>
              <span class="min-w-0 flex-1 truncate text-muted" title={ev.note ?? ''}>{ev.note ?? '—'}</span>
              <span class="shrink-0 font-mono text-[10px] text-muted">{ev.agentModel}</span>
              <span class="shrink-0 font-mono text-[10px] text-muted">{relativeTime(ev.createdAt)}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  {/if}
</Panel>

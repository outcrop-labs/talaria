<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { cn } from '@/lib/cn'
  import { getJson } from '@/lib/fetch-json'

  interface GuardData {
    config: { mode: string; checks: Record<string, boolean>; minConfidence: number; policedHosts: string[]; coach: boolean }
    stats: { total: number; byCheck: Record<string, number> }
    findings: Array<{ id: string; caller: string; model: string; check: string; severity: string; confidence: number; message: string; snippet: string; createdAt: string }>
    rules: Array<{ id: string; label: string; severity: string; defaultOn: boolean }>
  }

  // Confab guardrail — a structural check on model output at the gateway. Observe
  // mode records findings out-of-band; annotate/strict act on them.
  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['guardrails'],
    queryFn: (): Promise<GuardData> => getJson<GuardData>('/api/admin/guardrails'),
    refetchInterval: 30_000,
  }))
  const data = $derived(query.data)
  const cfg = $derived(data?.config)
  const save = async (patch: Partial<GuardData['config']>) => {
    if (!cfg) return
    const body = { ...cfg, ...patch }
    await fetch('/api/admin/guardrails', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    await qc.invalidateQueries({ queryKey: ['guardrails'] })
  }
  const rules = $derived(data?.rules ?? [])
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="Confab guard"
    info={"A structural check on every model output at the gateway: fabricated tool claims, invented links, outage stories, leaked secrets and PII. Observe records findings; annotate flags the reply; strict also redacts. Flagged content never re-enters an agent’s context."}
  />
  {#if query.isPending}
    <!-- Everything here seeds from the query (mode, slider, coach toggle,
         rules) — hold the full control area so no false defaults flash. -->
    <div class="flex flex-wrap items-center gap-4">
      <Skeleton class="h-8 w-40" />
      <Skeleton class="h-4 w-40 rounded-full" delay={0.12} />
      <Skeleton class="h-4 w-44 rounded-full" delay={0.24} />
    </div>
    <div class="mt-3 space-y-1.5">
      {#each Array.from({ length: 6 }, (_, i) => i) as i (i)}
        <div class="flex items-center gap-2 py-0.5">
          <Skeleton class="h-4 w-4" delay={i * 0.1} />
          <Skeleton class="h-3 w-48 rounded-full" delay={i * 0.1} />
        </div>
      {/each}
    </div>
    <div class="mt-4">
      <SkeletonRows rows={3} />
    </div>
  {:else if !data}
    <!-- "0 findings" and mode=observe are both defaults this component
         invents when `data` is missing — over a failed read that reads as a
         guard that is quietly running and finding nothing. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load the guardrail configuration"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="flex flex-wrap items-center gap-4">
      <div class="flex items-center gap-2">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Mode</span>
        <Select size="sm" value={cfg?.mode ?? 'observe'} onchange={(e) => void save({ mode: e.currentTarget.value })} class="w-40">
          <option value="off">Off</option>
          <option value="observe">Observe</option>
          <option value="annotate">Annotate</option>
          <option value="strict">Strict</option>
        </Select>
      </div>
      <label class="flex items-center gap-2 text-xs text-muted" title="Findings below this confidence are dropped">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Min confidence</span>
        <input
          type="range" min={0} max={1} step={0.05}
          value={cfg?.minConfidence ?? 0.5}
          disabled={cfg?.mode === 'off'}
          onchange={(e) => void save({ minConfidence: Number(e.currentTarget.value) })}
          class="w-28 accent-accent"
        />
        <span class="w-8 font-mono text-[11px] text-fg">{((cfg?.minConfidence ?? 0.5) * 100).toFixed(0)}%</span>
      </label>
      <Checkbox
        class="gap-2"
        title="Repeated findings become templated behavioral notes in the agent's rendered soul (counts + advice only, never the flagged content). Applies on the next fleet render."
        checked={cfg?.coach ?? false}
        disabled={cfg?.mode === 'off'}
        onChange={(checked) => void save({ coach: checked })}
        label="Coach agents from findings"
      />
      <div class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{data?.stats.total ?? 0} findings</div>
    </div>
    <div class="mt-3 space-y-1.5">
      {#each rules as c (c.id)}
        {#snippet ruleLabel()}
          {c.label}
          {#if data?.stats.byCheck[c.id]}<span class="text-[11px] text-muted">· {data.stats.byCheck[c.id]}</span>{/if}
        {/snippet}
        <Checkbox
          class="gap-2 text-sm text-fg"
          checked={cfg?.checks[c.id] ?? c.defaultOn}
          disabled={cfg?.mode === 'off'}
          onChange={(checked) => cfg && void save({ checks: { ...cfg.checks, [c.id]: checked } })}
          label={ruleLabel}
        />
      {/each}
    </div>
    {#if data?.findings && data.findings.length > 0}
      <div class="mt-4">
        <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Recent findings</div>
        <div class="max-h-48 divide-y divide-line-subtle overflow-y-auto rounded-md border border-line">
          {#each data.findings.slice(0, 20) as f (f.id)}
            <div class="flex items-start gap-2 px-2 py-1.5 text-xs transition-colors hover:bg-hover">
              <span class={cn('shrink-0 rounded px-1 font-mono text-[10px] uppercase tracking-[0.05em]', f.severity === 'high' ? 'text-danger' : 'text-warning')}>{f.check.replace(/_/g, ' ')}</span>
              <span class="min-w-0 flex-1 truncate text-muted" title={f.snippet}>{f.snippet || f.message}</span>
              <span class="shrink-0 font-mono text-[10px] text-muted">{f.model}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  {/if}
</Panel>

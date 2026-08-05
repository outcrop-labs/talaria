<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { getJson } from '@/lib/fetch-json'

  // The automated QA judge — an advisory reliability gate. When a ticket hits
  // quality_review, a judge model reviews the agent's work and posts a verdict.
  type JudgeData = { config: { enabled: boolean; model: string | null; mode?: 'advisory' | 'enforcing' }; models: string[] }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['judge-config'],
    queryFn: (): Promise<JudgeData> => getJson<JudgeData>('/api/admin/judge'),
  }))
  const data = $derived(query.data)
  const savedFlash = useSavedFlash()
  const enabled = $derived(data?.config.enabled ?? false)
  const model = $derived(data?.config.model ?? '')
  const mode = $derived(data?.config.mode ?? 'enforcing')

  const save = async (patch: { enabled?: boolean; model?: string | null; mode?: 'advisory' | 'enforcing' }) => {
    const body = { enabled: patch.enabled ?? enabled, model: patch.model !== undefined ? patch.model : model, mode: patch.mode ?? mode }
    const r = await fetch('/api/admin/judge', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
    if (r.ok) {
      await qc.invalidateQueries({ queryKey: ['judge-config'] })
      savedFlash.flash()
    }
  }
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="QA judge"
    info="When an agent hands a ticket to quality review, a judge model reviews the reported work and posts a verdict (pass / revise / escalate) with specific issues. Enforcing: bad submissions never sit in QA — revise verdicts bounce straight back to the agent with the issues (capped, then a human takes over). Advisory: verdicts only, the human decides. Pick a strong model for the sharpest review."
  />
  {#if query.isPending}
    <!-- The checkbox and model select seed from the query — hold the whole
         control row so nothing renders unchecked and then flips. -->
    <div class="flex flex-wrap items-center gap-3">
      <Skeleton class="h-4 w-4" />
      <Skeleton class="h-3 w-52 rounded-full" delay={0.12} />
      <Skeleton class="h-8 w-64" delay={0.24} />
    </div>
  {:else if !data}
    <!-- The controls seed from the response, so a failed read renders the
         judge as OFF — and the first click would then save it off for real. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load the judge configuration"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="flex flex-wrap items-center gap-3">
      <Checkbox
        class="gap-2 text-sm text-fg"
        checked={enabled}
        onChange={(checked) => void save({ enabled: checked })}
        label="Run the judge on quality review"
      />
      <Segmented
        options={[
          { id: 'enforcing', label: 'Enforcing' },
          { id: 'advisory', label: 'Advisory' },
        ] as const}
        value={mode}
        onChange={(m) => void save({ mode: m })}
      />
      <div class="flex items-center gap-2">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model</span>
        <Select size="sm" value={model} onchange={(e) => void save({ model: e.currentTarget.value || null })} class="w-64" disabled={!enabled}>
          <option value="">Default (self-hosted)</option>
          {#each data?.models ?? [] as m (m)}<option value={m}>{m}</option>{/each}
          {#if model && !(data?.models ?? []).includes(model)}<option value={model}>{model}</option>{/if}
        </Select>
      </div>
      {#if savedFlash.saved}<span class="text-xs text-success">Saved</span>{/if}
    </div>
  {/if}
  <p class="mt-3 text-[11px] text-muted">Per-board override lives on each board (enforcing / advisory / off); boards on "inherit" follow this stance.</p>
</Panel>

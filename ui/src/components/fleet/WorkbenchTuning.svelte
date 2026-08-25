<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import ModelIdPicker from '@/components/fleet/ModelIdPicker.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { getList } from '@/lib/fetch-json'
  import type { AgentDef, patchAgentMeta } from '@/lib/fleet-defs'
  import { slide } from '@/lib/motion'
  import { useModels } from '@/lib/muse.svelte'
  import { EFFORTS, type WorkbenchProfileLite } from './workbench'

  // The per-agent knobs: WHICH harness this agent runs, and which model each
  // effort level means for it (blank = the org-wide Workbench roles).
  let {
    def,
    profile,
    save,
  }: {
    def: AgentDef
    profile: WorkbenchProfileLite
    save: (patch: Parameters<typeof patchAgentMeta>[1]) => Promise<void>
  } = $props()

  const modelsQuery = useModels()
  const models = $derived(Array.isArray(modelsQuery.data) ? [] : (modelsQuery.data?.models ?? []))
  // Labels come from the harness REGISTRY (builtin + app-shipped + custom).
  // GET /api/workbench/harnesses is 200 `{ harnesses }` or an auth failure —
  // no 404. The registry is only a LABEL source here (`labelOf` falls back to
  // the slug), so a failure degrades the copy rather than the control; it still
  // has to say so instead of silently showing raw slugs.
  const registryQuery = createQuery(() => ({
    queryKey: ['workbench-harnesses'],
    queryFn: (): Promise<Array<{ slug: string; label: string }>> =>
      getList<{ slug: string; label: string }>('/api/workbench/harnesses', 'harnesses'),
  }))
  const registry = $derived(registryQuery.data ?? [])
  const registryFailed = $derived(registryQuery.isError && registryQuery.data === undefined)
  const labelOf = (slug: string) => registry.find((h) => h.slug === slug)?.label ?? slug
  const chosen = $derived(def.workbenchHarness && profile.harnesses.includes(def.workbenchHarness) ? def.workbenchHarness : '')
</script>

<div class="mt-2 space-y-1.5">
  <div class="flex flex-wrap items-center gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Harness</span>
    <Select size="sm" value={chosen} onchange={(e) => void save({ workbenchHarness: e.currentTarget.value || null })} class="w-40">
      <option value="">Auto ({profile.harnesses[0] ? labelOf(profile.harnesses[0]) : 'none'})</option>
      {#each profile.harnesses as h (h)}
        <option value={h}>{labelOf(h)}</option>
      {/each}
    </Select>
    <InfoTip text="The coding tool this agent's workbench jobs run. Auto uses the profile's first harness. The effort models below are handed to it in its own syntax." />
  </div>
  {#if registryFailed}
    <div transition:slide={{ duration: 150 }}>
      <QueryError
        variant="inline"
        error={registryQuery.error}
        title="Harness names unavailable; showing raw slugs"
        onRetry={() => void registryQuery.refetch()}
      />
    </div>
  {/if}
  <div class="flex flex-wrap items-center gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Effort</span>
    {#each EFFORTS as [key, label] (key)}
      <span class="flex items-center gap-1">
        <span class="text-xs text-muted">{label}</span>
        <ModelIdPicker
          models={models.map((m) => m.id)}
          value={def.workbenchModels?.[key] ?? null}
          onChange={(model) => void save({ workbenchModels: { [key]: model } })}
          emptyLabel="org default"
          class="w-44"
        />
      </span>
    {/each}
    <InfoTip text="What low / medium / high effort means for THIS agent. Blank follows the org-wide Workbench model roles on /models. Agents pick the effort; these decide the model." />
  </div>
</div>

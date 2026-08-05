<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { getJson } from '@/lib/fetch-json'

  // ── Model Roles — which model handles each class of activity ────────────────
  interface ModelRoleRow {
    role: string
    label: string
    hint: string
    wired: boolean
  }
  type ModelRolesData = { roles: ModelRoleRow[]; assignments: Record<string, string>; models: string[] }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['model-roles'],
    queryFn: (): Promise<ModelRolesData> => getJson<ModelRolesData>('/api/admin/model-roles'),
  }))
  const assign = async (role: string, model: string | null) => {
    await fetch('/api/admin/model-roles', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ role, model }),
    })
    await qc.invalidateQueries({ queryKey: ['model-roles'] })
  }
</script>

{#if !query.data}
  <!-- Card skeleton matching the resolved layout: title bar + label/select rows
       — but only while it is genuinely still loading. -->
  <Panel>
    {#if query.isError}
      <QueryError
        variant="compact"
        error={query.error}
        title="Could not load model roles"
        onRetry={() => void query.refetch()}
      />
    {:else}
      <Skeleton class="mb-4 h-4 w-24 rounded-full" />
      <div class="space-y-4">
        {#each Array.from({ length: 6 }, (_, i) => i) as i (i)}
          <div class="flex items-center gap-3">
            <div class="min-w-0 flex-1 space-y-1.5">
              <Skeleton class="h-3 w-40 rounded-full" delay={i * 0.1} />
              <Skeleton class="h-2.5 w-64 rounded-full" delay={i * 0.1 + 0.06} />
            </div>
            <Skeleton class="h-8 w-56 shrink-0" delay={i * 0.1} />
          </div>
        {/each}
      </div>
    {/if}
  </Panel>
{:else}
  {@const data = query.data}
  <Panel>
    <SectionHeader
      title="Model roles"
      info="Which model handles each class of activity. Unset = auto (a sensible pick from what's registered). Agents' own brains are configured per agent and unaffected."
    />
    <ul class="divide-y divide-line">
      {#each data.roles as r (r.role)}
        <li class="flex items-center gap-3 py-2.5">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2 font-sans text-sm text-fg">
              {r.label}
              {#if !r.wired}<Chip title="This slot takes effect when its surface lands.">reserved</Chip>{/if}
            </div>
            <div class="font-sans text-xs text-muted">{r.hint}</div>
          </div>
          <Select
            size="sm"
            class="w-56 shrink-0"
            value={data.assignments[r.role] ?? ''}
            onchange={(e) => void assign(r.role, e.currentTarget.value || null)}
          >
            <option value="">Auto</option>
            {#each data.models as m (m)}
              <option value={m}>
                {m}
              </option>
            {/each}
          </Select>
        </li>
      {/each}
    </ul>
  </Panel>
{/if}

<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import CapabilityTags from '@/components/models/CapabilityTags.svelte'
  import { assignmentNotice } from '@/components/models/fitness'
  import { useModelCapabilities } from '@/components/models/fitness-queries'

  // ── Model Roles — which model handles each class of activity ────────────────
  interface ModelRoleRow {
    role: string
    label: string
    hint: string
    wired: boolean
  }
  /** Audit 1.6: a role assignment whose model is KNOWN not to be able to do the
   *  work. The server sends only real gaps — an unprobed model produces
   *  nothing, because unknown is not a lack — so anything here is worth a line
   *  of the admin's attention. */
  interface RoleIssue {
    role: string
    model: string
    missing: string[]
    note: string
  }
  type ModelRolesData = {
    roles: ModelRoleRow[]
    assignments: Record<string, string>
    models: string[]
    issues: RoleIssue[]
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['model-roles'],
    queryFn: (): Promise<ModelRolesData> => getJson<ModelRolesData>('/api/admin/model-roles'),
  }))
  const noteFor = $derived(new Map((query.data?.issues ?? []).map((i) => [i.role, i.note])))
  // Capability facts (the tags) and the last fitness run's per-slot bands (the
  // warning). `issues` above is the server's capability half — a capability
  // recorded FALSE. This adds the run half: a slot this model TESTED unfit for.
  // Both are advisory and neither blocks; see `assignmentNotice`.
  const capsQuery = useModelCapabilities()
  const rowFor = (model: string | undefined) => capsQuery.data?.models.find((m) => m.id === model)
  const noticeFor = (role: string, model: string | undefined) =>
    model
      ? assignmentNotice({
          entry: capsQuery.data?.index[model],
          slotKey: `role:${role}`,
          capabilityNote: noteFor.get(role) ?? null,
        })
      : null
  const assign = async (role: string, model: string | null) => {
    await fetch('/api/admin/model-roles', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ role, model }),
    })
    // The refetch is what surfaces the unfit warning for the pick just made.
    // Nothing here blocks or reverts the assignment: the admin is told, and the
    // choice stands. They may know something the capability probe does not.
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
        {@const assigned = data.assignments[r.role]}
        {@const notice = noticeFor(r.role, assigned)}
        <li class="py-2.5">
          <div class="flex items-center gap-3">
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2 font-sans text-sm text-fg">
                {r.label}
                {#if !r.wired}<Chip title="This slot takes effect when its surface lands.">reserved</Chip>{/if}
                <!-- No `title` here on purpose: the sentence is already visible
                     below the row, and a tooltip repeating it is noise. -->
                {#if notice}<Chip tone="warn">unfit</Chip>{/if}
              </div>
              <div class="font-sans text-xs text-muted">{r.hint}</div>
              <!-- What is KNOWN about the model actually assigned here. Three
                   states: measured yes, measured no, never measured — and the
                   third is not the second. -->
              <CapabilityTags class="mt-1" row={rowFor(assigned)} />
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
          </div>
          <!-- The notice row: a sentence, not a validation error. It appears
               under the row it belongs to and leaves the Select untouched. -->
          {#if notice}
            <p transition:slide={{ duration: 150 }} class="mt-1.5 max-w-prose font-sans text-xs text-warning">{notice.text}</p>
          {/if}
        </li>
      {/each}
    </ul>
  </Panel>
{/if}

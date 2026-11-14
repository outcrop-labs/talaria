<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'

  // The automated QA judge — an advisory reliability gate. When a ticket hits
  // quality_review, a judge model reviews the agent's work and posts a verdict.
  type JudgeData = { config: { enabled: boolean; model: string | null; mode?: 'advisory' | 'enforcing' }; models: string[] }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['judge-config'],
    queryFn: (): Promise<JudgeData> => getJson<JudgeData>('/api/admin/judge'),
  }))
  const data = $derived(query.data)
  const autosave = useAutosave()
  const enabled = $derived(data?.config.enabled ?? false)
  const model = $derived(data?.config.model ?? '')
  const mode = $derived(data?.config.mode ?? 'enforcing')

  const save = async (patch: { enabled?: boolean; model?: string | null; mode?: 'advisory' | 'enforcing' }) => {
    const body = { enabled: patch.enabled ?? enabled, model: patch.model !== undefined ? patch.model : model, mode: patch.mode ?? mode }
    try {
      await putJson<{ config: JudgeData['config'] }>('/api/admin/judge', body)
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['judge-config'] })
    autosave.flash(Object.keys(patch)[0])
  }
</script>

<SettingsSection
  title="Quality review"
  description="A judge model checks an agent's work when a ticket reaches quality review."
  info="The judge posts a verdict of pass, revise or escalate, with the specific issues. A strong model gives the sharpest review."
>
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load the judge configuration" errorVariant="compact">
      {#snippet skeleton()}
        <!-- The toggle and model select seed from the query — hold the rows so
             nothing renders off and then flips. -->
        <div class="px-5 py-4"><SkeletonRows rows={3} /></div>
      {/snippet}
      {#snippet children(_data)}
        <!-- The controls seed from the response, so a failed read must never
             render them: it would show the judge as OFF, and the first click
             would then save it off for real. QueryState's error branch holds
             that. -->
        <SettingsRow label="Run the judge" hint="Review every ticket handed to quality review." saved={autosave.saved('enabled')}>
          <Toggle checked={enabled} onChange={(checked) => void save({ enabled: checked })} title="Run the judge on quality review" />
        </SettingsRow>
        <SettingsRow
          label="When work needs changes"
          hint={mode === 'enforcing' ? 'The ticket goes straight back to the agent with the issues.' : 'The verdict is posted and a person decides.'}
          info="Enforcing sends work back to the agent a limited number of times, then a person takes over. Each board can override this, and boards set to inherit follow it."
          saved={autosave.saved('mode')}
        >
          <Segmented
            options={[
              { id: 'enforcing', label: 'Enforcing' },
              { id: 'advisory', label: 'Advisory' },
            ] as const}
            value={mode}
            onChange={(m) => void save({ mode: m })}
          />
        </SettingsRow>
        <SettingsRow label="Judge model" hint="The model that writes the verdict." as="label" saved={autosave.saved('model')}>
          <Select size="sm" value={model} onchange={(e) => void save({ model: e.currentTarget.value || null })} class="w-[280px]" disabled={!enabled}>
            <option value="">Default (self-hosted)</option>
            {#each data?.models ?? [] as m (m)}<option value={m}>{m}</option>{/each}
            {#if model && !(data?.models ?? []).includes(model)}<option value={model}>{model}</option>{/if}
          </Select>
        </SettingsRow>
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>

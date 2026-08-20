<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { savePreferredModel, useModels, usePreferredModel } from '@/lib/muse.svelte'

  // The model that powers AI drafting (souls, skills, memories, crons) across
  // Talaria. Picks from the models the caller may use (the server filters by the
  // admin's member allowlist), shown with pretty names + what each is good at —
  // normal users shouldn't need to know what "qwen/qwen3-14b" means.

  const qc = useQueryClient()
  // Both queries are KEPT, not flattened: a failed /api/models renders as one
  // option called "Default", and a failed /api/profile renders the saved pick
  // as "Default" too. Either one is this control lying about what is set, so
  // the failure gets the region instead of the combobox.
  const catalogQuery = useModels()
  const prefsQuery = usePreferredModel()
  const catalog = $derived(catalogQuery.data)
  const prefs = $derived(prefsQuery.data)
  const catalogLoading = $derived(catalogQuery.isPending)
  const prefsLoading = $derived(prefsQuery.isPending)
  const failed = $derived(catalogQuery.isError ? catalogQuery : prefsQuery.isError ? prefsQuery : null)
  const savedFlash = useSavedFlash()
  let error = $state<string | null>(null)
  // The catalog as the server spells it: "<endpoint>/<model>" pins, plus bare
  // pool ids where a model is served by more than one endpoint. This was
  // filtered to bare ids once, back when the catalog emitted both spellings —
  // now it emits bare ids only for pools, so the filter hid every model.
  const models = $derived(catalog?.models ?? [])

  const save = async (model: string | null) => {
    error = null
    const r = await savePreferredModel(model)
    if (r && typeof r === 'object' && 'error' in r && r.error) error = String(r.error)
    await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
    await qc.invalidateQueries({ queryKey: ['gateway-models'] })
    savedFlash.flash()
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4">
  <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Preferred model</label>
  {#if catalogLoading || prefsLoading}
    <!-- Mounting the combobox before both queries land makes its value flip
         from Default to the saved pick — hold with a select-shaped shimmer. -->
    <Skeleton class="h-11 w-full" />
  {:else if failed}
    <QueryError
      variant="inline"
      error={failed.error}
      title="Could not load your preferred model"
      onRetry={() => {
        void catalogQuery.refetch()
        void prefsQuery.refetch()
      }}
    />
  {:else}
    <Combobox
      options={[
        {
          value: '',
          label: `Default${catalog?.effective && !prefs?.preferredModel ? ` (${catalog.effective})` : ''}`,
          sub: 'Let Talaria pick a sensible model',
        },
        ...models.map((m) => ({ value: m.id, label: m.label ?? m.id, sub: m.blurb || m.id })),
      ]}
      selected={[prefs?.preferredModel ?? '']}
      onChange={([v]) => void save(v || null)}
      placeholder="Pick a model"
    />
  {/if}
  <p class="mt-1 text-xs text-muted">
    Powers AI drafting across Talaria: souls, skills, memories, schedules.
    {#if savedFlash.saved && !error}<span class="ml-2 text-success">Saved</span>{/if}
    {#if error}<span class="ml-2 text-danger">{error}</span>{/if}
  </p>
</div>

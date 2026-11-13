<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { anyPending, firstError } from '@/components/ui/query-state'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { savePreferredEffort, savePreferredModel, useModels, useProfilePrefs } from '@/lib/muse.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'

  // The model that powers AI drafting (souls, skills, memories, crons) across
  // Talaria. Picks from the models the caller may use (the server filters by the
  // admin's member allowlist), shown with pretty names + what each is good at —
  // normal users shouldn't need to know what "qwen/qwen3-14b" means.
  //
  // Below it, when the picked model publishes reasoning-effort levels, the
  // user's PLATFORM DEFAULT effort: the level every effort-enabled surface
  // starts from wherever the model in play supports it (agent DMs, the
  // assistant panel). A model with no published ladder shows no control —
  // there is nothing to default, and the requests carry no effort at all.

  const qc = useQueryClient()
  // Both queries are KEPT, not flattened: a failed /api/models renders as one
  // option called "Default", and a failed /api/me renders the saved pick
  // as "Default" too. Either one is this control lying about what is set, so
  // the failure gets the region instead of the combobox.
  const catalogQuery = useModels()
  const prefsQuery = useProfilePrefs()
  const catalog = $derived(catalogQuery.data)
  const prefs = $derived(prefsQuery.data)
  // Both reads gate the picker, so every flag of both is read before they are
  // combined (query-state.ts, anyLoading): `catalogLoading || prefsLoading`
  // left the prefs' flag unread while the catalog loaded.
  const loading = $derived(anyPending(catalogQuery, prefsQuery))
  const failed = $derived(firstError(catalogQuery, prefsQuery))
  const savedFlash = useSavedFlash()
  let error = $state<string | null>(null)
  let effortError = $state<string | null>(null)
  const effortFlash = useSavedFlash()
  // The catalog as the server spells it: "<endpoint>/<model>" pins, plus bare
  // pool ids where a model is served by more than one endpoint. This was
  // filtered to bare ids once, back when the catalog emitted both spellings —
  // now it emits bare ids only for pools, so the filter hid every model.
  const models = $derived(catalog?.models ?? [])

  // The levels the PICKED model publishes — the default-effort control's whole
  // option list, and the reason it renders only for models that have one.
  // Getter form: the pick can change under the query.
  // Kept whole and read through its getter: destructured, the levels would
  // be the query's answer at mount and never move.
  const modelEfforts = useModelEfforts(() => prefs?.preferredModel ?? null)
  const efforts = $derived(modelEfforts.efforts)
  // A saved default the picked model cannot honor (it was set against another
  // model, or the metadata changed) reads as auto here — inert, never an error.
  const effectiveEffort = $derived(
    prefs?.preferredEffort && efforts.includes(prefs.preferredEffort) ? prefs.preferredEffort : '',
  )

  const sentence = (level: string) => level.charAt(0).toUpperCase() + level.slice(1)

  const save = async (model: string | null) => {
    error = null
    const r = await savePreferredModel(model)
    if (r && typeof r === 'object' && 'error' in r && r.error) error = String(r.error)
    await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
    await qc.invalidateQueries({ queryKey: ['gateway-models'] })
    savedFlash.flash()
  }

  const saveEffort = async (effort: string) => {
    effortError = null
    // '' is the AUTO row: saving it CLEARS the preference — every model runs
    // at its own default again, which is what picking auto means.
    const r = await savePreferredEffort(effort || null)
    if (r && typeof r === 'object' && 'error' in r && r.error) effortError = String(r.error)
    await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
    effortFlash.flash()
  }
</script>

<SettingsRow label="Preferred model">
  {#snippet hint()}
    {#if error}
      <span class="text-danger" role="alert">{error}</span>
    {:else}
      Used when Talaria drafts for you.
    {/if}
  {/snippet}
  {#if loading}
    <!-- Mounting the combobox before both queries land makes its value flip
         from Default to the saved pick. Hold with a select-shaped shimmer. -->
    <Skeleton class="h-9 w-[280px]" />
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
    {#if savedFlash.saved && !error}<span class="font-sans text-[13px] leading-5 text-success">Saved</span>{/if}
    <Combobox
      size="sm"
      class="w-[280px]"
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
</SettingsRow>

{#if !failed && !loading && efforts.length > 0}
  <!-- ONLY FOR MODELS THAT PUBLISH A LADDER: there is no default to set on a
       model with no effort setting, and rendering one would promise the
       requests something they cannot carry. -->
  <SettingsRow label="Reasoning effort">
    {#snippet hint()}
      {#if effortError}
        <span class="text-danger" role="alert">{effortError}</span>
      {:else}
        Your starting level in agent chats and the assistant panel.
      {/if}
    {/snippet}
    {#if effortFlash.saved && !effortError}<span class="font-sans text-[13px] leading-5 text-success">Saved</span>{/if}
    <!-- '' is the model's own default: saving it clears the preference. -->
    <Combobox
      size="sm"
      class="w-[280px]"
      searchable={false}
      options={[{ value: '', label: 'Auto', sub: 'The model decides' }, ...efforts.map((level) => ({ value: level, label: sentence(level) }))]}
      selected={[effectiveEffort]}
      onChange={([v]) => void saveEffort(v ?? '')}
    />
  </SettingsRow>
{/if}

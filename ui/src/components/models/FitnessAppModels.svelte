<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import ModelIdPicker from '@/components/fleet/ModelIdPicker.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import type { SlotView } from './fitness'

  // THE APP COLUMNS' MODEL SETTINGS. Each harness an enabled app ships is a
  // column in the matrix above and a setting here: the model it runs on, the
  // slot `app:<slug>/<harnessId>`. Read beside the column that measures it,
  // because "which model holds this harness" and "which model should run it"
  // are one decision. An assignment wins before the harness's own chain (its
  // role, the utility model); Auto leaves the chain to decide. Saved on pick,
  // the way every model assignment on this page is.
  let {
    slots,
    assigned,
    models,
  }: {
    /** The matrix's app columns. */
    slots: SlotView[]
    /** slot key → assigned model. */
    assigned: Record<string, string>
    /** Assignable gateway model ids. */
    models: string[]
  } = $props()

  const qc = useQueryClient()
  // These pickers are the tab's only editable controls, so with an app
  // harness on the page its header says changes save as they are made.
  const autosave = useAutosave(() => slots.length > 0)

  const assign = async (slot: string, model: string | null) => {
    try {
      await putJson<{ ok: true }>('/api/admin/app-harness-models', { slot, model })
    } catch (e) {
      toastError('Could not assign the model', e)
      return
    }
    autosave.flash(slot)
    await qc.invalidateQueries({ queryKey: ['model-fitness'] })
  }
</script>

{#if slots.length > 0}
  <SettingsSection
    title="App harness models"
    description="The model each app harness runs on. Auto follows the harness's own chain."
    info="An assignment wins before the harness's declared role and the utility model. The matching column in the matrix says how each model scored on the harness's own fixtures."
  >
    <SettingsGroup>
      {#each slots as slot (slot.key)}
        <SettingsRow label={slot.label} saved={autosave.saved(slot.key)}>
          {#snippet hint()}<span class="font-mono text-xs" title={slot.hint}>{slot.key}</span>{/snippet}
          <ModelIdPicker
            {models}
            value={assigned[slot.key] ?? null}
            onChange={(m) => void assign(slot.key, m)}
            emptyLabel="Auto"
            emptySub="The harness's own chain"
            class="w-[280px]"
          />
        </SettingsRow>
      {/each}
    </SettingsGroup>
  </SettingsSection>
{/if}

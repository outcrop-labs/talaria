<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
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
  let saving = $state<string | null>(null)

  const assign = async (slot: string, model: string | null) => {
    saving = slot
    try {
      await putJson<{ ok: true }>('/api/admin/app-harness-models', { slot, model })
      await qc.invalidateQueries({ queryKey: ['model-fitness'] })
    } catch (e) {
      toastError('Could not assign the model', e)
    } finally {
      saving = null
    }
  }
</script>

{#if slots.length > 0}
  <div class="mt-4 rounded-md border border-line p-3">
    <SectionHeader
      title="App harness models"
      info="Each harness an enabled app ships runs on the model picked here, before its own chain. Auto lets the harness's declared role and the utility model decide. The column above says how each model scored on the harness's own fixtures."
    />
    <ul class="divide-y divide-line">
      {#each slots as slot (slot.key)}
        <li class="flex flex-wrap items-center gap-3 py-2">
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm text-fg">{slot.label}</div>
            <div class="truncate font-mono text-[11px] text-muted" title={slot.hint}>{slot.key}</div>
          </div>
          {#if saving === slot.key}
            <Chip tone="neutral" class="shrink-0">saving</Chip>
          {/if}
          <ModelIdPicker
            {models}
            value={assigned[slot.key] ?? null}
            onChange={(m) => void assign(slot.key, m)}
            emptyLabel="Auto"
            emptySub="the harness's own chain"
            class="w-64 max-w-full"
          />
        </li>
      {/each}
    </ul>
  </div>
{/if}

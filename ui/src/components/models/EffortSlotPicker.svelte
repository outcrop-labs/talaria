<script lang="ts">
  import EffortPicker from '@/components/chat/EffortPicker.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'

  // The effort dial on a Models-view slot row (a Model role, a platform
  // agent).
  //
  // THREE STATES, all visible — the dial being invisible when a model is
  // unassigned or ladder-less read as "the feature does not exist", which is
  // how it shipped the first time:
  //    model assigned, publishes levels → the EffortPicker chip
  //    model assigned, no levels (answer landed) → a quiet "no levels" label,
  //      so the absence is legible rather than a hole in the row
  //    Auto (no model) → nothing; there is no model to ask a level of, and the
  //      select beside this says so already
  //
  // A COMPONENT RATHER THAN A CALL IN THE PARENT'S each-loop because
  // `useModelEfforts` is a query hook: one per row means one query per
  // assigned model, which is exactly the cache shape wanted.
  let {
    model,
    value,
    onChange,
    disabled = false,
  }: {
    /** The model assigned to the slot; null/undefined (Auto) renders nothing. */
    model: string | null | undefined
    /** The stored preference ('' = the model's own default). */
    value: string
    onChange: (effort: string | null) => void
    disabled?: boolean
  } = $props()

  const { efforts, isLoading } = useModelEfforts(() => model ?? null)
  const settled = $derived(!!model && !isLoading)
</script>

{#if model && efforts.length > 0}
  <EffortPicker {efforts} value={value ?? ''} onChange={(v) => onChange(v || null)} {disabled} />
{:else if settled}
  <span
    class="px-1 font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim"
    title="This model publishes no reasoning-effort levels"
  >
    no levels
  </span>
{/if}

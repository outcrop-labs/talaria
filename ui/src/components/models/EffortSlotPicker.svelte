<script lang="ts">
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'

  // The effort dial on a Models-view slot row (a Model role, a platform
  // agent).
  //
  // THREE STATES, all visible — the dial being invisible when a model is
  // unassigned or ladder-less read as "the feature does not exist", which is
  // how it shipped the first time:
  //    model assigned, publishes levels → the effort chip
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

  // The effort chip's shape: its rows, the rung the stored level sits on ('' is
  // the model's own default — no rung), and the ingress row that means it.
  const effort = $derived(value ?? '')
  const effortOptions = $derived(efforts.map((level) => ({ value: level, label: level })))
  const effortMeter = $derived({ total: efforts.length, lit: Math.max(0, efforts.indexOf(effort) + 1) })
  const effortAuto = { value: '', label: 'auto', sub: 'model default' }
</script>

{#if model && efforts.length > 0}
  <ComposerPicker
    chipVariant="primary"
    value={effort}
    label={effort || 'auto'}
    options={effortOptions}
    autoOption={effortAuto}
    meter={effortMeter}
    searchable={false}
    menuClass="min-w-48"
    title="Reasoning effort for this reply"
    menuLabel="Reasoning effort"
    {disabled}
    onChange={(v) => onChange(v || null)}
  />
{:else if settled}
  <span
    class="px-1 font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim"
    title="This model publishes no reasoning-effort levels"
  >
    no levels
  </span>
{/if}
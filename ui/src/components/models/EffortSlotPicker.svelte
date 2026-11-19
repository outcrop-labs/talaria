<script lang="ts">
  import Combobox from '@/components/ui/Combobox.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'

  // The effort level on a Models-view row (a model role, a platform agent).
  //
  // It renders only for a model that publishes levels, the same rule the
  // Profile page's effort picker follows: there is no level to set on a model
  // with no ladder, or on Auto, where there is no model to ask.
  //
  // A COMPONENT RATHER THAN A CALL IN THE PARENT'S each-loop because
  // `useModelEfforts` is a query hook: one per row means one query per
  // assigned model, which is exactly the cache shape wanted.
  let {
    model,
    value,
    onChange,
    disabled = false,
    class: className,
  }: {
    /** The model assigned to the slot; null/undefined (Auto) renders nothing. */
    model: string | null | undefined
    /** The stored preference ('' = the model's own default). */
    value: string
    onChange: (effort: string | null) => void
    disabled?: boolean
    class?: string
  } = $props()

  // Kept whole and read through its getter: destructured, the levels would be
  // the query's state at mount, frozen.
  const modelEfforts = useModelEfforts(() => model ?? null)
  const efforts = $derived(modelEfforts.efforts)

  const sentence = (level: string) => level.charAt(0).toUpperCase() + level.slice(1)
</script>

{#if model && efforts.length > 0}
  <!-- '' is the model's own default: picking it clears the preference. A stored
       level the model no longer publishes reads as that default. -->
  <Combobox
    size="sm"
    class={className}
    searchable={false}
    {disabled}
    options={[{ value: '', label: 'Auto effort', sub: 'The model decides' }, ...efforts.map((level) => ({ value: level, label: sentence(level) }))]}
    selected={[efforts.includes(value) ? value : '']}
    onChange={([v]) => onChange(v || null)}
  />
{/if}

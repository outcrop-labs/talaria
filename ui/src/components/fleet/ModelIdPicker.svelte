<script lang="ts">
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ComboOption } from '@/components/ui/combobox'
  import type { ControlSize } from '@/components/ui/control'

  // The searchable picker for a BARE model id — "which model runs this"
  // wherever a setting names one id from a list (slot assignments, workbench
  // effort models, fitness candidates, a reranker's catalog). The native
  // <select> these replace is fine at five models and collapses at fifty: no
  // search, browser chrome, and a first option that quietly means null. This
  // is the house Combobox with the one convention every model pick shares —
  // a leading null-valued row whose LABEL says what null means (Auto, org
  // default, no adversary), so clearing the pick is a first-class choice
  // rather than a missing one.
  //
  // Sister of ModelPicker, which picks endpoint+model as a ModelTarget for
  // agent configs; this one answers the places that store a single id.
  let {
    models,
    value,
    onChange,
    emptyLabel,
    emptySub,
    placeholder = 'Pick a model',
    size = 'sm',
    bare = false,
    class: className,
  }: {
    /** Pickable model ids. */
    models: string[]
    /** The current id, or null/undefined for the empty row. */
    value: string | null | undefined
    onChange: (model: string | null) => void
    /** The leading null-valued row's label — what picking null MEANS. Omit
     *  for required picks: every option is then a model. */
    emptyLabel?: string
    emptySub?: string
    placeholder?: string
    size?: ControlSize
    /** Trigger without its own frame, for a bordered control cluster. */
    bare?: boolean
    class?: string
  } = $props()

  // The current pick stays selectable after it leaves the list — the trigger
  // would otherwise show a value the menu refuses to re-choose — labelled so
  // the staleness is legible. Same rule as ModelPicker.
  const options = $derived.by(() => {
    const opts: ComboOption[] = emptyLabel ? [{ value: '', label: emptyLabel, sub: emptySub }] : []
    if (value && !models.includes(value)) opts.push({ value, label: value, sub: 'no longer listed' })
    for (const m of models) opts.push({ value: m, label: m })
    return opts
  })
</script>

<Combobox
  {options}
  selected={[value ?? '']}
  onChange={([v]) => onChange(v || null)}
  {placeholder}
  {size}
  {bare}
  class={className}
/>

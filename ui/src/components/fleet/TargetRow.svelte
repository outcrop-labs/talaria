<script lang="ts">
  import { X } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import ModelPicker from '@/components/fleet/ModelPicker.svelte'
  import { useModelEfforts } from '@/lib/model-efforts.svelte'
  import type { LlmEndpoint, ModelTarget } from '@/lib/fleet-defs'

  // One endpoint+model row of the agent config form (optionally named — alias
  // tiers). Its own file (in React it had to live at module scope: an inline
  // component would get a new identity every render and drop input focus).
  let {
    endpoints,
    value,
    onChange,
    onRemove,
    namePlaceholder,
    name,
    onName,
    allowEffort = false,
  }: {
    endpoints: LlmEndpoint[]
    value: ModelTarget
    onChange: (t: ModelTarget) => void
    onRemove?: () => void
    namePlaceholder?: string
    name?: string
    onName?: (n: string) => void
    /** Offer the default-effort pick beside the model (main + tiers). Off for
     *  fallback rows: a fallback is an emergency backup, and defaulting its
     *  reasoning is a setting nobody asked for on a path that should not run. */
    allowEffort?: boolean
  } = $props()

  const epClass = $derived(endpoints.find((e) => e.name === value.endpoint)?.class ?? 'cloud')

  // The picked model's published levels, keyed by the endpoint-qualified
  // catalog spelling — the row holds endpoint + model separately, and the
  // server resolves exactly this shape. The control appears only for models
  // that publish a ladder: a model without one has no effort to default.
  // Kept whole and read through its getters: destructured, the levels would be
  // the query's answer at mount and never move.
  const modelEfforts = useModelEfforts(() => (value.endpoint && value.model ? `${value.endpoint}/${value.model}` : null))
  const efforts = $derived(modelEfforts.efforts)
  // A model swap can retire the saved level (tiers spread `{ ...x, ...t }`, so
  // the old effort survives the model change in the form state). Reset to
  // unset rather than saving a level the new model would refuse — judged
  // against the model's ANSWER only. While the read is in flight `efforts` is
  // `[]`, and on a fresh page every saved level would read as retired and be
  // cleared before its levels arrived.
  $effect(() => {
    if (allowEffort && modelEfforts.answered && value.effort && !efforts.includes(value.effort)) onChange({ ...value, effort: null })
  })

  // '' is the model's own default: no level saved.
  const effort = $derived(value.effort ?? '')
  const named = (level: string) => `${level.charAt(0).toUpperCase()}${level.slice(1)} effort`
</script>

<div class="flex items-center gap-2">
  {#if onName !== undefined}
    <Input value={name ?? ''} oninput={(e) => onName?.(e.currentTarget.value)} placeholder={namePlaceholder} aria-label="Tier name" size="sm" class="w-32 shrink-0" />
  {/if}
  <ModelPicker {endpoints} {value} {onChange} size="sm" class="min-w-0 flex-1" />
  {#if allowEffort && efforts.length > 0}
    <!-- The default effort for THIS target's model: the pick conversations
         with the agent (base or this tier) start from when nobody chose one. -->
    <Select
      size="sm"
      value={effort}
      aria-label="Reasoning effort"
      title="The reasoning effort a conversation starts from"
      onchange={(e) => onChange({ ...value, effort: e.currentTarget.value || null })}
      class="w-36 shrink-0"
    >
      <option value="">Default effort</option>
      {#each efforts as level (level)}<option value={level}>{named(level)}</option>{/each}
    </Select>
  {/if}
  <span class="flex w-[88px] shrink-0 justify-end"><Chip>{epClass === 'local' ? 'Self-hosted' : 'Cloud'}</Chip></span>
  {#if onRemove}
    <IconButton title="Remove" onclick={onRemove}><X size={16} /></IconButton>
  {/if}
</div>

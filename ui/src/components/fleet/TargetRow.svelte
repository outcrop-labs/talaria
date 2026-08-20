<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import EffortPicker from '@/components/chat/EffortPicker.svelte'
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
  const { efforts } = useModelEfforts(() => (value.endpoint && value.model ? `${value.endpoint}/${value.model}` : null))
  // A model swap can retire the saved level (tiers spread `{ ...x, ...t }`, so
  // the old effort survives the model change in the form state). Reset to
  // unset rather than saving a level the new model would refuse.
  $effect(() => {
    if (allowEffort && value.effort && !efforts.includes(value.effort)) onChange({ ...value, effort: null })
  })
</script>

<div class="flex items-center gap-2">
  {#if onName !== undefined}
    <Input value={name ?? ''} oninput={(e) => onName?.(e.currentTarget.value)} placeholder={namePlaceholder} size="sm" class="w-28 shrink-0" />
  {/if}
  <ModelPicker {endpoints} {value} {onChange} size="sm" class="min-w-0 flex-1" />
  {#if allowEffort && efforts.length > 0}
    <!-- The default effort for THIS target's model — the pick conversations
         with the agent (base or this tier) start from when nobody chose one.
         Same chip + ladder the composers use, so the setting and the thing it
         sets look like one feature. -->
    <EffortPicker {efforts} value={value.effort ?? ''} onChange={(v) => onChange({ ...value, effort: v || null })} />
  {/if}
  <span class={`w-10 shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] ${epClass === 'local' ? 'text-success' : 'text-accent'}`}>
    {epClass}
  </span>
  {#if onRemove}
    <Button variant="ghost" size="sm" class="shrink-0" onclick={onRemove}>
      ✕
    </Button>
  {/if}
</div>

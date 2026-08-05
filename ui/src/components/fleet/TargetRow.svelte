<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import ModelPicker from '@/components/fleet/ModelPicker.svelte'
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
  }: {
    endpoints: LlmEndpoint[]
    value: ModelTarget
    onChange: (t: ModelTarget) => void
    onRemove?: () => void
    namePlaceholder?: string
    name?: string
    onName?: (n: string) => void
  } = $props()

  const epClass = $derived(endpoints.find((e) => e.name === value.endpoint)?.class ?? 'cloud')
</script>

<div class="flex items-center gap-2">
  {#if onName !== undefined}
    <Input value={name ?? ''} oninput={(e) => onName?.(e.currentTarget.value)} placeholder={namePlaceholder} size="sm" class="w-28 shrink-0" />
  {/if}
  <ModelPicker {endpoints} {value} {onChange} size="sm" class="min-w-0 flex-1" />
  <span class={`w-10 shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] ${epClass === 'local' ? 'text-success' : 'text-accent'}`}>
    {epClass}
  </span>
  {#if onRemove}
    <Button variant="ghost" size="sm" class="shrink-0" onclick={onRemove}>
      ✕
    </Button>
  {/if}
</div>

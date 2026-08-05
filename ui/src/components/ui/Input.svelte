<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import { controlSizes, type ControlSize } from './control'

  interface Props extends Omit<HTMLInputAttributes, 'size'> {
    size?: ControlSize
    ref?: HTMLInputElement | null
  }

  let {
    size = 'md',
    class: className,
    value = $bindable(),
    ref = $bindable(null),
    ...rest
  }: Props = $props()
</script>

<!-- The one text input. Reuse everywhere — do not re-style inputs inline. -->
<input
  bind:this={ref}
  bind:value
  class={cn(
    controlSizes[size],
    // What users TYPE is content, not chrome — sans. Callers needing mono
    // (keys, slugs, config) pass font-[var(--font-mono)]; cn() resolves it.
    // Spec §8: raised tile bg, hairline border, radius 6, gold focus ring.
    'w-full rounded-md border border-line bg-[var(--theme-input)] px-3 font-sans text-sm text-fg outline-none transition-colors',
    'placeholder:text-muted focus:border-accent focus:ring-2 focus:ring-accent-soft',
    className,
  )}
  {...rest}
/>

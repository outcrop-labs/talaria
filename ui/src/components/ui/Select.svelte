<script lang="ts">
  import type { HTMLSelectAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import { controlSizes, type ControlSize } from './control'

  interface Props extends Omit<HTMLSelectAttributes, 'size'> {
    size?: ControlSize
    ref?: HTMLSelectElement | null
  }

  let {
    size = 'md',
    class: className,
    children,
    value = $bindable(),
    ref = $bindable(null),
    ...rest
  }: Props = $props()
</script>

<!-- The one select. Reuse for status/priority/agent/role controls. -->
<select
  bind:this={ref}
  bind:value
  class={cn(
    controlSizes[size],
    // Spec §8: raised tile bg, hairline border, radius 6, gold focus ring.
    'rounded-md border border-line bg-[var(--theme-input)] px-2.5 font-sans text-sm text-fg outline-none transition-colors',
    'focus:border-accent focus:ring-2 focus:ring-accent-soft',
    className,
  )}
  {...rest}
>
  {@render children?.()}
</select>

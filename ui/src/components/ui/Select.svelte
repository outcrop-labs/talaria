<script lang="ts">
  import type { HTMLSelectAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import { useUiVoice } from '@/lib/ui-voice'
  import { controlSizes, disabledControl, type ControlSize } from './control'

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

  const sans = useUiVoice() === 'sans'
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
    disabledControl,
    sans && 'select-sans-caret',
    className,
  )}
  {...rest}
>
  {@render children?.()}
</select>

<style>
  /* In the settings area every dropdown shows the Combobox's chevron, so a
     native select beside one does not read as a different control. A select
     takes no child icon, so the chevron is two hairline strokes painted in the
     muted token, placed where the Combobox draws its 16px icon. The padding
     keeps the width the native arrow gave the control. Global, so a select
     outside the area carries no scoping class and renders as it always has. */
  :global(select.select-sans-caret) {
    appearance: none;
    padding-right: 30px;
    background-image:
      linear-gradient(45deg, transparent calc(50% - 0.55px), var(--theme-muted) calc(50% - 0.55px), var(--theme-muted) calc(50% + 0.55px), transparent calc(50% + 0.55px)),
      linear-gradient(-45deg, transparent calc(50% - 0.55px), var(--theme-muted) calc(50% - 0.55px), var(--theme-muted) calc(50% + 0.55px), transparent calc(50% + 0.55px));
    background-size: 4px 4px;
    background-position:
      right 18px center,
      right 14px center;
    background-repeat: no-repeat;
  }
</style>

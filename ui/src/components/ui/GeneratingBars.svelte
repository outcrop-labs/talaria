<script lang="ts">
  import { cn } from '@/lib/cn'
  import { MOTIF, type GeneratingVariant } from './generating'

  // One row of 3×12 motif bars. Colour rides on `currentColor` (set a text-*
  // class): line-tone for content placeholders, accent gold for signal.
  let {
    bars = 5,
    variant = 'scan',
    delay = 0,
    step = 0.08,
    class: className,
  }: {
    bars?: number
    variant?: GeneratingVariant
    /** Stagger offset for the whole row, seconds. */
    delay?: number
    /** Per-bar stagger, seconds (the cascade across the row). */
    step?: number
    class?: string
  } = $props()
</script>

<span aria-hidden="true" class={cn('inline-flex items-center gap-[3px]', className)}>
  {#each Array.from({ length: bars }) as _, i (i)}
    <span
      class={cn('gd-bar', MOTIF[variant])}
      style:animation-delay={`${(delay + i * step).toFixed(2)}s`}
      style:animation-fill-mode="backwards"
    ></span>
  {/each}
</span>

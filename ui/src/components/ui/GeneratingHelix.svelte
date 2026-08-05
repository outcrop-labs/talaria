<script lang="ts">
  import { cn } from '@/lib/cn'
  import { HELIX_TONES } from './generating'

  // CONTEXT HELIX (spec §9): the reasoning / assistant-generating loop — a
  // staggered multi-colour bar helix (chart-1 / gold / chart-2) orbiting on
  // the standard budget. Reduced motion degrades it to the two-state pulse
  // (styles.css) with the same colours.
  let { bars = 7, class: className }: { bars?: number; class?: string } = $props()
</script>

<span aria-hidden="true" class={cn('inline-flex items-center gap-[3px]', className)}>
  {#each Array.from({ length: bars }) as _, i (i)}
    <span
      class={cn('gd-bar gd-helix', HELIX_TONES[i % HELIX_TONES.length])}
      style:animation-delay={`${(i * 0.12).toFixed(2)}s`}
      style:animation-fill-mode="backwards"
    ></span>
  {/each}
</span>

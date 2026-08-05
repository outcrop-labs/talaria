<script lang="ts">
  import type { Snippet } from 'svelte'
  import { autoHeight } from '@/lib/motion'

  // Animated-resize container: when the content's height changes — a wizard
  // step swap, a form section appearing — the box glides to the new height
  // instead of snapping (ANIMATIONS.md: "if it resizes, it animates"). The
  // inner div exists so the action has a stable single child to measure.
  let {
    duration = 220,
    class: className,
    children,
  }: {
    duration?: number
    class?: string
    children: Snippet
  } = $props()
</script>

<div use:autoHeight={{ duration }} class={className}>
  <div>{@render children()}</div>
</div>

<script lang="ts">
  import { cn } from '@/lib/cn'

  // Loading skeletons (Skeleton / SkeletonRows / SkeletonCard) — the shape of
  // the coming content on the ambient MONITOR BREATHE motif (spec §9):
  // surface-toned blocks breathing slowly on the --motion-ambient budget.
  // Same idiom as Generating (gd-* motif bars) but for FETCHES: use these
  // instead of a blank pane or a "Loading" string wherever a query hasn't
  // resolved yet. Rule of thumb: the skeleton should roughly match the layout
  // it's standing in for, so the swap doesn't jump.

  // One breathing block. Size it with class (h-*, w-*).
  let { class: className, delay = 0 }: { class?: string; delay?: number } = $props()
</script>

<div
  aria-hidden="true"
  class={cn('gd-breathe rounded-md bg-card2', className)}
  style:animation-delay="{delay}s"
></div>

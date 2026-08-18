<script lang="ts">
  import { cn } from '@/lib/cn'
  import { attachSkeletonField } from '@/lib/skeleton-static'

  // Loading skeletons (Skeleton / SkeletonRows / SkeletonCard) — the shape of
  // the coming content, rendered as SIGNAL STATIC: a dithered dot field on the
  // house Bayer grid, noise re-rolled at 8Hz around a steady mean. An
  // instrument that hasn't acquired its signal yet. See lib/skeleton-static.ts
  // for why static and not a sweep, and for the shared ticker.
  //
  // Use these instead of a blank pane or a "Loading" string wherever a query
  // hasn't resolved yet. Rule of thumb: the skeleton should roughly match the
  // layout it's standing in for, so the swap doesn't jump.
  //
  // `Generating` is for MODEL output being written; this is for FETCHES.
  //
  // One block of the field. Size it with class (h-*, w-*); the class's border
  // radius is the mask, so `rounded-full` gives a capsule or a circle. There
  // is no per-block delay or stagger: static has no phase, and the lattice is
  // page-wide, so neighbouring blocks are windows onto ONE field.
  let { class: className }: { class?: string } = $props()

  let host = $state<HTMLDivElement | null>(null)
  let canvas = $state<HTMLCanvasElement | null>(null)

  $effect(() => {
    const h = host
    const c = canvas
    if (!h || !c) return
    return attachSkeletonField(h, c)
  })
</script>

<div bind:this={host} aria-hidden="true" class={cn('relative rounded-md', className)}>
  <canvas bind:this={canvas} class="pointer-events-none absolute inset-0 h-full w-full"></canvas>
</div>

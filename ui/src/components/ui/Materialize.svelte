<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { fade, listStagger } from '@/lib/motion'

  // Skeleton → content, as one motion: while loading, `count` copies of an
  // ITEM-SHAPED skeleton render in the exact container the real items will
  // use; when data lands the skeletons fade out IN PLACE while the items
  // stagger in over them. Both branches occupy the same grid cell, so there
  // is no layout jump — the list materializes, it doesn't swap.
  //
  //   <Materialize loading={q.isLoading} count={5} class="space-y-2">
  //     {#snippet skeleton()}<ServerCardSkeleton />{/snippet}
  //     {#each servers as s (s.id)}<ServerCard {s} />{/each}
  //   </Materialize>
  //
  // The skeleton snippet must MIRROR the item's silhouette (frame, avatar
  // block, line widths, pill row) — that likeness is what sells the
  // materialize. Sketch it with the Skeleton primitives at the item's own
  // dimensions; a generic SkeletonRows here defeats the point.
  //
  // `class` styles BOTH branch containers (the list's grid/stack classes) so
  // the skeletons lay out exactly like the items. The content branch carries
  // the standard list cascade (rule of thumb) — don't add another
  // use:listStagger inside.
  let {
    loading,
    count = 5,
    class: className,
    skeleton,
    children,
  }: {
    loading: boolean
    /** How many item-skeletons to sketch — match the typical viewport fill. */
    count?: number
    /** Container classes shared by both branches (e.g. 'grid gap-4 xl:grid-cols-3'). */
    class?: string
    skeleton: Snippet<[number]>
    children: Snippet
  } = $props()
</script>

<!-- min-w-0 on the wrapper AND the branch cells: grid items default to
     min-width:auto, which forbids shrinking below content width — inside a
     sized rail that meant rows laid out at full text width and the ancestor
     clipped them, so `truncate` down in the items never engaged. -->
<div class="grid min-w-0">
  {#if loading}
    <div class={cn('col-start-1 row-start-1 min-w-0', className)} out:fade={{ duration: 220 }} use:listStagger>
      {#each Array(count) as _, i (i)}
        {@render skeleton(i)}
      {/each}
    </div>
  {:else}
    <div class={cn('col-start-1 row-start-1 min-w-0', className)} in:fade={{ duration: 200 }} use:listStagger>
      {@render children()}
    </div>
  {/if}
</div>

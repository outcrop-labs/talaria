<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { PROSE_WIDTHS } from './knowledge.svelte'

  // See PROSE_WIDTHS in knowledge.svelte.ts — matches the doc/space editor
  // layout (breadcrumb, toolbar with icon + title + buttons, centered prose
  // column) so the swap to real content doesn't jump.
  let { breadcrumb = false, bars = 10 }: { breadcrumb?: boolean; bars?: number } = $props()
</script>

<div aria-hidden="true" class="flex h-full min-h-0 flex-col">
  {#if breadcrumb}
    <div class="border-b border-line-subtle px-6 pb-2 pt-3">
      <Skeleton class="h-2.5 w-44 rounded-full" />
    </div>
  {/if}
  <div class="flex items-center gap-3 border-b border-line-subtle px-6 py-4">
    <Skeleton class="h-7 w-7 shrink-0" />
    <Skeleton class="h-5 w-64 max-w-[40%] rounded-full" delay={0.08} />
    <span class="ml-auto flex shrink-0 gap-2">
      <Skeleton class="h-7 w-20" delay={0.16} />
      <Skeleton class="h-7 w-28" delay={0.24} />
    </span>
  </div>
  <div class="mx-auto w-full max-w-[46rem] flex-1 space-y-3.5 px-6 py-8">
    {#each PROSE_WIDTHS.slice(0, bars) as w, i (i)}
      <div style:width={w}>
        <Skeleton class="h-3 w-full rounded-full" delay={i * 0.08} />
      </div>
    {/each}
  </div>
</div>

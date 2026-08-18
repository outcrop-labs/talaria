<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'

  /** Doc-page skeleton: toolbar row + prose block, shaped like the editor it
   *  stands in for. The SAME skeleton covers both loading phases (doc lookup,
   *  then the artifact fetch) so the pane never double-swaps mid-load. */
  const widths = ['100%', '94%', '88%', '97%', '82%', '91%', '60%']
</script>

<div aria-hidden="true" class="flex min-h-0 min-w-0 flex-1 flex-col">
  <div class="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
    <Skeleton class="h-2.5 w-20 rounded-full" />
    <div class="min-w-0 flex-1">
      <Skeleton class="h-3 w-1/2 rounded-full" />
    </div>
    <Skeleton class="h-7 w-24 shrink-0" />
    <Skeleton class="h-7 w-14 shrink-0" />
  </div>
  <div class="min-h-0 flex-1 space-y-3 overflow-hidden p-6">
    <Skeleton class="h-5 w-2/3 rounded-full" />
    {#each widths as w, i (i)}
      <div style:width={w}>
        <Skeleton class="h-2.5 w-full rounded-full" />
      </div>
    {/each}
  </div>
</div>

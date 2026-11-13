<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'

  /**
   * No stagger and no delay: static has no direction, and a sweep would imply a
   * completion the fetch cannot promise (docs/UI-CONVENTIONS.md, Loading).
   */
</script>

<div class="overflow-hidden rounded-xl border border-line bg-panel">
  <div class="px-5 pb-6 pt-5">
    <Skeleton class="h-5 w-56 rounded-full" />
    <Skeleton class="mt-2.5 h-3 w-36 rounded-full" />
    <div class="mt-5 max-w-[68ch]"><SkeletonRows rows={3} /></div>
  </div>
  <div class="grid grid-cols-2 border-t border-line sm:grid-cols-4">
    {#each [0, 1, 2, 3] as i (i)}
      <div class="px-5 py-4">
        <Skeleton class="h-6 w-8 rounded" />
        <Skeleton class="mt-2 h-3 w-20 rounded-full" />
      </div>
    {/each}
  </div>
</div>

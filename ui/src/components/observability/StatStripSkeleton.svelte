<script lang="ts">
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'

  // The loading shape of a tab's `StatStrip`: the same card and cell heights,
  // so the numbers land without moving what is under them.
  let { cells = 4 }: { cells?: 3 | 4 } = $props()
</script>

<StatStrip columns={cells}>
  {#each Array.from({ length: cells }) as _, i (i)}
    <div aria-hidden="true" class="border-b border-r border-line-subtle px-5 py-4">
      <Skeleton class="my-0.5 h-3 w-20 rounded-full" />
      <Skeleton class="mb-1 mt-2 h-6 w-16 rounded" />
      <Skeleton class="my-1 h-3 w-28 rounded-full" />
    </div>
  {/each}
</StatStrip>

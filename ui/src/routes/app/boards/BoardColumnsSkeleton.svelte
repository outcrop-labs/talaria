<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'

  // Same column skeleton as the board page, so the redirect lands seamlessly.
</script>

<div class="grid h-full grid-cols-2 gap-3 overflow-hidden p-6 sm:grid-cols-4">
  {#each [0, 1, 2, 3] as c (c)}
    <div class="space-y-3">
      <Skeleton class="h-3 w-20 rounded-full" delay={c * 0.1} />
      {#each [0, 1, 2] as r (r)}
        <SkeletonCard delay={c * 0.1 + r * 0.15} />
      {/each}
    </div>
  {/each}
</div>

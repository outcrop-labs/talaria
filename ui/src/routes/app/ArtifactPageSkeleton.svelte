<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'

  // Loading stand-in matching the artifact editor layout: toolbar (icon + title
  // + buttons) over a centered prose column, so the swap to content doesn't jump.
  const PROSE_WIDTHS = ['100%', '92%', '97%', '86%', '95%', '73%', '100%', '90%', '96%', '58%']
</script>

<div aria-hidden="true" class="flex h-full min-h-0 flex-col">
  <div class="flex items-center gap-3 border-b border-line-subtle px-6 py-4">
    <Skeleton class="h-7 w-7 shrink-0" />
    <Skeleton class="h-5 w-64 max-w-[40%] rounded-full" />
    <span class="ml-auto flex shrink-0 gap-2">
      <Skeleton class="h-7 w-20" />
      <Skeleton class="h-7 w-28" />
    </span>
  </div>
  <div class="mx-auto w-full max-w-[var(--read-width)] flex-1 space-y-3.5 px-6 py-8">
    {#each PROSE_WIDTHS as w, i (i)}
      <div style:width={w}>
        <Skeleton class="h-3 w-full rounded-full" />
      </div>
    {/each}
  </div>
</div>

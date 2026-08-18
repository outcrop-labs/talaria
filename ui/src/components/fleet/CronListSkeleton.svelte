<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'

  // The shape of the jobs list while it loads: dot + name + schedule + actions.
  let { rows = 3 }: { rows?: number } = $props()
</script>

<div aria-hidden="true" class="divide-y divide-line rounded-lg border border-line">
  {#each Array.from({ length: rows }) as _, i (i)}
    <div class="flex items-center gap-2.5 px-3.5 py-3">
      <!-- Fixed geometry, so nothing about it is uncertain — a flat rail, not a skeleton (UI-CONVENTIONS, Loading). -->
      <div class="h-2 w-2 shrink-0 rounded-full bg-line"></div>
      <Skeleton class="h-3 w-36 rounded-full" />
      <Skeleton class="h-2.5 w-28 rounded-full" />
      <span class="ml-auto flex shrink-0 items-center gap-1">
        <Skeleton class="h-7 w-7 rounded-md" />
        <Skeleton class="h-7 w-7 rounded-md" />
        <Skeleton class="h-7 w-7 rounded-md" />
      </span>
    </div>
  {/each}
</div>

<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'

  // The shape of the schedules card while it loads: dot, name, cadence, next run.
  let { rows = 3 }: { rows?: number } = $props()
</script>

<div aria-hidden="true" class="overflow-hidden rounded-lg border border-line bg-panel [&>*+*]:border-t [&>*+*]:border-line-subtle">
  {#each Array.from({ length: rows }) as _, i (i)}
    <div class="flex min-h-[52px] items-center gap-4 px-5 py-3">
      <!-- Fixed geometry, so nothing about it is uncertain: a flat dot, not a skeleton (UI-CONVENTIONS, Loading). -->
      <div class="h-1.5 w-1.5 shrink-0 rounded-full bg-line"></div>
      <Skeleton class={`h-3.5 rounded-full ${['w-44', 'w-36', 'w-52'][i % 3]}`} />
      <Skeleton class="ml-auto h-3 w-28 rounded-full" />
      <Skeleton class="h-3 w-24 rounded-full" />
      <Skeleton class="h-5 w-9 rounded-full" />
    </div>
  {/each}
</div>

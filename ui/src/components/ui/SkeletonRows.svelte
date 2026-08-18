<script lang="ts">
  import { cn } from '@/lib/cn'
  import Skeleton from './Skeleton.svelte'

  // N list rows: optional leading dot/avatar circle + a text bar. For sidebars,
  // queues, tables — anything row-shaped. (See Skeleton.svelte for the
  // material.)
  let {
    rows = 4,
    avatar = false,
    class: className,
  }: { rows?: number; avatar?: boolean; class?: string } = $props()

  const widths = ['70%', '85%', '55%', '78%', '62%', '88%']
</script>

<div aria-hidden="true" class={cn('space-y-3', className)}>
  {#each Array.from({ length: rows }) as _, i (i)}
    <div class="flex items-center gap-2.5">
      {#if avatar}<Skeleton class="h-6 w-6 shrink-0 rounded-full" />{/if}
      <div style:width={widths[i % widths.length]}>
        <Skeleton class="h-2.5 w-full rounded-full" />
      </div>
    </div>
  {/each}
</div>

<script lang="ts">
  import { cn } from '@/lib/cn'

  // Chip meter: 3×12 bars, ~2px gap — lit bars gold, unlit raised-tone.
  let { total, lit, class: className }: { total: number; lit: number; class?: string } = $props()
</script>

<span aria-hidden="true" class={cn('flex items-center gap-[2px]', className)}>
  {#each Array.from({ length: total }) as _, i (i)}
    <span class={cn('h-3 w-[3px] rounded-full', i < lit ? 'bg-accent' : 'bg-raised')}></span>
  {/each}
</span>

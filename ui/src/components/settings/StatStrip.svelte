<script lang="ts">
  import type { Snippet } from 'svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { cn } from '@/lib/cn'

  // One card of stats side by side, divided by hairlines. Children are `Stat`s.
  // More than `columns` of them wrap to another row inside the same card.
  let {
    columns = 4,
    class: className,
    children,
  }: { columns?: 2 | 3 | 4; class?: string; children: Snippet } = $props()

  const COLUMNS = { 2: 'sm:grid-cols-2', 3: 'sm:grid-cols-3', 4: 'sm:grid-cols-2 lg:grid-cols-4' }
</script>

<Panel class={cn('overflow-hidden p-0', className)}>
  <!-- Each stat draws its own top and left hairline; the grid sits one pixel
       up and left so the outer ones fall under the card's edge. -->
  <div class={cn('-ml-px -mt-px grid grid-cols-1', COLUMNS[columns])}>
    {@render children()}
  </div>
</Panel>

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
  <!-- Each stat draws its own right and bottom hairline; the grid runs one
       pixel past the card's right and bottom edge so the outer ones are cut. -->
  <div class={cn('-mb-px -mr-px grid grid-cols-1', COLUMNS[columns])}>
    {@render children()}
  </div>
</Panel>

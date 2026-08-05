<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  /** The stage (main area) beside a rail. (See RailSurface.svelte for the
   *  page-archetype grammar this belongs to.) */
  let {
    header,
    children,
    class: className,
  }: { header?: Snippet; children: Snippet; class?: string } = $props()
</script>

<main class={cn('flex min-h-0 min-w-0 flex-1 flex-col', className)}>
  {@render header?.()}
  <div class="min-h-0 min-w-0 flex-1">{@render children()}</div>
</main>

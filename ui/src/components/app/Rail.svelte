<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  /** The list rail — ALWAYS on the left, always w-72. (See RailSurface.svelte
   *  for the page-archetype grammar this belongs to.) */
  let {
    title,
    actions,
    children,
    class: className,
  }: {
    title?: string
    /** Compact IconButtons; never a giant labeled button. */
    actions?: Snippet
    children: Snippet
    class?: string
  } = $props()
</script>

<aside class={cn('flex h-full w-72 shrink-0 flex-col border-r border-line bg-sidebar', className)}>
  {#if title || actions}
    <div class="flex h-12 shrink-0 items-center gap-1.5 border-b border-line px-4">
      {#if title}<span class="min-w-0 flex-1 truncate font-sans text-sm font-semibold text-fg">{title}</span>{/if}
      {@render actions?.()}
    </div>
  {/if}
  <div class="min-h-0 flex-1 overflow-y-auto p-3">{@render children()}</div>
</aside>

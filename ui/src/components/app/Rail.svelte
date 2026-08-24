<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  /** The list rail — ALWAYS on the left, always w-72. (See RailSurface.svelte
   *  for the page-archetype grammar this belongs to.) No title: naming the
   *  view is the top strip's job now. The header bar exists only to carry
   *  actions, and a rail with none of those starts straight at its content. */
  let {
    actions,
    children,
    class: className,
  }: {
    /** Compact IconButtons; never a giant labeled button. Right-aligned —
     *  they sat right when a title still filled the left. */
    actions?: Snippet
    children: Snippet
    class?: string
  } = $props()
</script>

<aside class={cn('flex h-full w-72 shrink-0 flex-col border-r border-line bg-sidebar', className)}>
  {#if actions}
    <div class="flex h-12 shrink-0 items-center gap-1.5 border-b border-line px-4">
      <div class="ml-auto flex items-center gap-1.5">{@render actions()}</div>
    </div>
  {/if}
  <div class="min-h-0 flex-1 overflow-y-auto p-3">{@render children()}</div>
</aside>

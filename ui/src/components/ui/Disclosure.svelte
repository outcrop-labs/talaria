<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'

  // A clean collapsible section — collapsed by default. Reuse for thinking traces,
  // tool calls, and any other secondary detail that shouldn't clutter the thread.
  // Mercury (spec §10): compact mono trigger row on a raised hairline card.
  let {
    title,
    icon,
    defaultOpen = false,
    class: className,
    children,
  }: {
    title: string | Snippet
    icon?: Snippet
    defaultOpen?: boolean
    class?: string
    children: Snippet
  } = $props()

  let open = $state(defaultOpen)
</script>

<div class={cn('overflow-hidden rounded-md border border-line bg-card', className)}>
  <button
    type="button"
    onclick={() => (open = !open)}
    class="group/disc flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left font-mono text-[11px] tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg"
  >
    <!-- The chevron is the affordance, so it gets brighter on hover rather than
         staying the same dim glyph the row's own text is. A row that expands and
         a row that does nothing looked identical until the pointer was on it. -->
    <span class={cn('text-[10px] transition-all duration-150 group-hover/disc:text-accent', open && 'rotate-90')}>▶</span>
    {@render icon?.()}
    <span class="min-w-0 flex-1 truncate">
      {#if typeof title === 'string'}{title}{:else}{@render title()}{/if}
    </span>
  </button>
  {#if open}
    <div transition:slide={{ duration: 150 }}>
      <div class="border-t border-line px-3 py-2">{@render children()}</div>
    </div>
  {/if}
</div>

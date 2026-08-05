<script lang="ts" generics="T extends string">
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import type { TabItem } from './tabs'

  // The one tab strip — Mercury tile tabs (boards `44Y-0`/`8A-0`): mono
  // uppercase labels, the active tab a raised hairline tile, dashed-gold
  // keyboard focus. Every tabbed surface uses this; nobody re-rolls it.
  let {
    items,
    value,
    onChange,
    class: className,
  }: {
    items: ReadonlyArray<TabItem<T>>
    value: T
    onChange: (id: T) => void
    class?: string
  } = $props()
</script>

<div class={cn('flex items-center gap-1', className)}>
  {#each items as t (t.id)}
    <button
      type="button"
      onclick={() => onChange(t.id)}
      class={cn(
        'flex h-7 items-center rounded-md border px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
        focusGold,
        value === t.id
          ? 'border-line bg-raised text-fg'
          : 'border-transparent text-muted hover:bg-hover hover:text-fg',
      )}
    >
      {#if typeof t.label === 'string'}{t.label}{:else}{@render t.label()}{/if}
    </button>
  {/each}
</div>

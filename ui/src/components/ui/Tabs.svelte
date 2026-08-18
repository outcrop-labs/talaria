<script lang="ts" generics="T extends string">
  import { cn } from '@/lib/cn'
  import DitherPool from './DitherPool.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { markCrossfade } from '@/lib/motion'
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

  // The raised tile is the mark that MOVES between tabs (ANIMATIONS.md): one
  // send/receive pair per strip. The button keeps a transparent border so the
  // hit target never changes size; the mark paints the hairline on top of it.
  const [sendMark, receiveMark] = markCrossfade()
</script>

<!-- `relative` so the pool has something to anchor to; it costs no layout.
     Tabs are wider and shorter than a segmented cell, so the field needs the
     steeper falloff to read as concentric rather than as two long bands. -->
<div class={cn('relative flex items-center gap-1', className)}>
  <DitherPool key={value} selector="[data-active='true']" falloff={3} />
  {#each items as t (t.id)}
    <button data-dither-fill="on"
      type="button"
      onclick={() => onChange(t.id)}
      data-active={value === t.id}
      class={cn(
        'relative flex h-7 items-center rounded-md border border-transparent px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
        focusGold,
        value === t.id ? 'text-fg' : 'text-muted hover:text-fg',
      )}
    >
      {#if value === t.id}
        <span
          aria-hidden="true"
          in:receiveMark={{ key: 'mark' }}
          out:sendMark={{ key: 'mark' }}
          class="absolute -inset-px rounded-md border border-line bg-raised"
        ></span>
      {/if}
      <span class="relative">
        {#if typeof t.label === 'string'}{t.label}{:else}{@render t.label()}{/if}
      </span>
    </button>
  {/each}
</div>

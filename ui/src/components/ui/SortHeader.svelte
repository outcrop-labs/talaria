<script lang="ts">
  import type { Component, Snippet } from 'svelte'
  import { ChevronDown, ChevronUp } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // The one sortable column head: the label dims to ink-dim until its column is
  // the sorted one, then it is fg with an arrow beside it, reversed on a
  // right-aligned column. Two surfaces hand-rolled exactly this (the board's
  // list and the artifact browser's list).
  //
  // It owns the AFFORDANCE, not the CYCLE, and that is deliberate: `onclick` is
  // the caller's sort function, because the two tables answer differently. The
  // board walks unsorted → asc → desc → unsorted and stores that per board; the
  // browser toggles asc/desc and never returns to unsorted (and opens its
  // "Modified" column newest-first, which no generic cycle can know). A shared
  // cycle here would silently hand one of them a state it has never had.
  //
  // The `<th>`/grid cell stays the caller's too — padding, alignment, sticky
  // and width classes differ per table — so this renders the button only, and
  // takes those classes via `class` (twMerge resolves what it conflicts with).
  let {
    active = false,
    dir = 'asc',
    arrows = [ChevronUp, ChevronDown],
    size = 12,
    onclick,
    class: className,
    children,
  }: {
    /** Is this the sorted column? */
    active?: boolean
    /** Which arrow the active column shows. */
    dir?: 'asc' | 'desc'
    /** `[ascending, descending]` glyphs: the board's 12px head uses chevrons,
     *  the browser's 10px one arrows. Pass the pair the table already draws —
     *  a component is not the place to restyle a head. */
    arrows?: [Component<any>, Component<any>]
    size?: number
    onclick: () => void
    class?: string
    children: Snippet
  } = $props()
</script>

<button
  type="button"
  {onclick}
  class={cn(
    'flex items-center gap-1 uppercase tracking-[0.08em] transition-colors hover:text-fg',
    active ? 'text-fg' : 'text-ink-dim',
    className,
  )}
>
  {@render children()}
  {#if active}
    {@const Arrow = dir === 'asc' ? arrows[0] : arrows[1]}
    <Arrow {size} />
  {/if}
</button>
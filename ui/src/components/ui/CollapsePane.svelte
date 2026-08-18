<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // The one collapsible pane. An always-mounted region that glides between an
  // expanded and a collapsed width (nav rail, assistant panel) — this is a
  // WIDTH transition on a live element, not a mount transition, so it lives
  // in CSS, with the house duration/curve owned here and nowhere else.
  // Content swaps instantly at the flip and clips during the glide
  // (overflow-hidden); callers render per `collapsed` as they already do.
  // Reduced motion: the width snaps (motion-reduce:transition-none).
  let {
    collapsed,
    width,
    collapsedWidth,
    tag = 'div',
    animate = true,
    style,
    class: className,
    children,
    ...rest
  }: {
    collapsed: boolean
    /** Expanded width class, e.g. 'w-[208px]'. May carry responsive variants
     *  and CSS vars (`min-[1400px]:w-[var(--panel-w)]`). */
    width: string
    /** Collapsed width class, e.g. 'w-16'. Both must resolve to fixed widths — auto can't glide. */
    collapsedWidth: string
    tag?: 'div' | 'nav' | 'aside' | 'section'
    /** Anything else lands on the element — presence listeners, data-*, aria-*.
     *  The pane is a plain box that animates its width; it has no reason to
     *  swallow the attributes a caller would put on the element it replaces. */
    [key: string]: unknown
    /** false while the user is drag-resizing: the width must track the
     *  pointer instantly, not chase it through the transition. */
    animate?: boolean
    style?: string
    class?: string
    children: Snippet
  } = $props()
</script>

<svelte:element
  this={tag}
  {style}
  class={cn(
    'overflow-hidden',
    animate && 'transition-[width] duration-300 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none',
    collapsed ? collapsedWidth : width,
    className,
  )}
  {...rest}
>
  {@render children()}
</svelte:element>

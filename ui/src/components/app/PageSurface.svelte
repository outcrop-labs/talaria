<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  /**
   * The second page archetype, as a component rather than as a comment.
   *
   * `RailSurface` was already a real component; this one existed only as prose
   * inside it ("centered page surfaces keep the `h-full overflow-y-auto p-8` +
   * `mx-auto max-w-5xl` shape"), and ten views hand-rolled it. They drifted to
   * four different widths, one view changed width between its own TABS, and the
   * layout skeleton was wider than the content it stood in for — so a cold load
   * settled narrower and every tab change slid sideways.
   *
   * A written convention cannot be enforced; a component can. There is
   * deliberately no `width` prop: an escape hatch here would be taken within a
   * week by whichever table felt cramped, and one view opting out is the whole
   * problem coming back. A surface that genuinely needs more room scrolls its
   * wide child horizontally (`overflow-x-auto`) rather than widening the page
   * under everything else.
   *
   *   <PageSurface>            most views: scroll owned here, p-8, centred
   *   <PageSurface flush>      the view paints its own padding (tab strips
   *                            that must span the full bleed)
   *   <PageSurface scroll={false}>
   *                            the view owns its own scroll container, e.g.
   *                            because a child is a sticky header
   *
   * `space-y-6` is NOT applied — section rhythm belongs to the view, and
   * several stagger their children with `use:staggerIn`, which needs to own the
   * element it is on.
   */
  let {
    children,
    flush = false,
    scroll = true,
    class: className,
    innerClass,
  }: {
    children: Snippet
    /** Drop the p-8 gutter; the view paints its own. */
    flush?: boolean
    /** Set false when the view owns its own scroll container. */
    scroll?: boolean
    /** On the outer scroll box. */
    class?: string
    /** On the centred column — where `use:staggerIn` targets and rhythm goes. */
    innerClass?: string
  } = $props()
</script>

<div class={cn('h-full min-h-0 min-w-0', scroll && 'overflow-y-auto', !flush && 'p-8', className)}>
  <!-- w-full alongside the max-width: without it the column is only as wide as
       its content, so a short page centres a narrow block and the panels no
       longer line up with the view you just came from — the same jank by a
       different route. -->
  <div class={cn('mx-auto w-full max-w-[var(--page-width)]', innerClass)}>
    {@render children()}
  </div>
</div>

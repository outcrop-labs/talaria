<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'

  // The §7 popover shell for CONTENT panels — DropdownMenu's mechanics with
  // the menu grammar stripped out. Before this, every non-menu popover
  // (pickers, swatch grids, small forms) hand-rolled the same four behaviors
  // and no two agreed: outside-click, Escape, what a scroll does, and how the
  // panel is positioned. This file is the one owner of those four decisions —
  //   · positioned FIXED from the trigger rect at open time, PORTALed to
  //     <body> (cards carry backdrop-filter, a stacking context — an
  //     absolutely-positioned sibling can never stack above neighboring cards,
  //     no z-index would save it)
  //   · outside mousedown and Escape close
  //   · a scroll either closes the panel (the context menu's rule — the anchor
  //     moved, and guessing where to is worse than reopening) or FOLLOWS it
  //     (`follow`, for triggers docked to fixed chrome like the composer,
  //     where repositioning is exact)
  // A trigger click toggles; the click is the popover's, never the row or
  // card underneath. Callers wanting a menu's item list have DropdownMenu;
  // anything else renders here as `content`. The popover census rule in
  // scripts/check-invariants.mjs keeps new engines from appearing.
  let {
    trigger,
    content,
    align = 'left',
    up = false,
    follow = false,
    offset = 4,
    class: className,
    open = $bindable(false),
  }: {
    /** Renders the trigger; the `open` param lets it style its active state.
     *  Its click toggles the popover. */
    trigger: Snippet<[boolean]>
    /** The panel body — any content, not just menu items. Receives `close`
     *  for rows that pick a value; clicks inside the panel never close it. */
    content: Snippet<[() => void]>
    /** Panel's left/right edge against the trigger's. */
    align?: 'left' | 'right'
    /** Open upward (triggers docked at the bottom of the viewport). */
    up?: boolean
    /** Reposition on scroll/resize instead of closing — for triggers anchored
     *  to fixed chrome, where the anchor rect is stable and following is
     *  exact. */
    follow?: boolean
    /** Gap between trigger and panel, px. */
    offset?: number
    class?: string
    /** Bindable, so a popover can be opened/closed from elsewhere (a sibling
     *  action, a keyboard shortcut) rather than only by its trigger. */
    open?: boolean
  } = $props()

  let ref = $state<HTMLDivElement | null>(null)
  let panelEl = $state<HTMLDivElement | null>(null)
  // Fixed-position style computed from the trigger at open time — see the
  // portal note above for why fixed + portal rather than absolute.
  let pos = $state<string | null>(null)

  const close = () => {
    open = false
  }

  function place(): string | null {
    const r = ref?.getBoundingClientRect()
    if (!r) return null
    const vert = up ? `bottom: ${window.innerHeight - r.top + offset}px` : `top: ${r.bottom + offset}px`
    const horiz = align === 'right' ? `right: ${Math.max(8, window.innerWidth - r.right)}px` : `left: ${Math.max(8, r.left)}px`
    return `position: fixed; z-index: 80; ${vert}; ${horiz}`
  }

  function toggle() {
    if (open) return close()
    pos = place()
    if (pos) open = true
  }

  // In follow mode a viewport resize recentres the panel on the (stable)
  // anchor; in close mode there is nothing to do — any scroll closes.
  function onResize() {
    if (open && follow) pos = place()
  }

  function onDocMousedown(e: MouseEvent) {
    if (!open) return
    const t = e.target as Node
    if (!ref?.contains(t) && !panelEl?.contains(t)) close()
  }

  function onDocKeydown(e: KeyboardEvent) {
    if (open && e.key === 'Escape') close()
  }

  // A scroll inside the panel (a scrolling option list) is not the page
  // moving — ignored. Any other scroll either follows (recompute from the
  // trigger's live rect) or closes, matching the context menu's rule.
  function onDocScroll(e: Event) {
    if (!open) return
    if (panelEl?.contains(e.target as Node)) return
    if (follow) pos = place()
    else close()
  }
</script>

<svelte:window onresize={onResize} />
<svelte:document onmousedown={onDocMousedown} onkeydown={onDocKeydown} onscrollcapture={onDocScroll} />

<div bind:this={ref} class="relative">
  <!-- The trigger click is the popover's — never the row/card underneath. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <span
    onclick={(e) => {
      e.stopPropagation()
      toggle()
    }}
  >
    {@render trigger(open)}
  </span>
  {#if open && pos}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <div
      bind:this={panelEl}
      use:portal
      style={pos}
      onclick={(e) => e.stopPropagation()}
      class={cn(
        popPanel,
        up ? (align === 'right' ? 'origin-bottom-right' : 'origin-bottom-left') : align === 'right' ? 'origin-top-right' : 'origin-top-left',
        className,
      )}
      in:pop={POPOVER}
      out:fade={QUICK}
    >
      {@render content(close)}
    </div>
  {/if}
</div>

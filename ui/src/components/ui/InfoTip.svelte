<script lang="ts">
  import { Info } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'

  // An ⓘ that explains an area on hover — the home for the sentence that used
  // to sit under a section header cluttering it. Keep the text to one or two
  // sentences; anything longer belongs in docs.
  //
  // The bubble PORTALS to <body> and is positioned from the trigger's rect, the
  // same way DropdownMenu/Combobox/ContextMenu do. It used to be an absolutely
  // positioned sibling, which was fine until its container scrolled: every
  // modal now scrolls its content region, and an absolute bubble is clipped at
  // that region's edge — so the tips nearest the bottom of a dialog, exactly
  // where the longest explanations sit, got cut in half.
  let { text, class: className }: { text: string; class?: string } = $props()

  let open = $state(false)
  let ref = $state<HTMLSpanElement | null>(null)
  let pos = $state<string | null>(null)

  const WIDTH = 256 // w-64, needed here because the clamp has to do the maths
  const MARGIN = 8

  function show() {
    const r = ref?.getBoundingClientRect()
    if (!r) return
    // Centred on the trigger, then pulled back inside the viewport rather than
    // allowed to run off the edge — a tip on a right-hand control would.
    const left = Math.min(Math.max(r.left + r.width / 2 - WIDTH / 2, MARGIN), window.innerWidth - WIDTH - MARGIN)
    // Below by default; above when there isn't room, so a tip on the last row
    // of a dialog is still readable.
    const below = window.innerHeight - r.bottom
    const vert = below < 96 ? `bottom: ${window.innerHeight - r.top + 6}px` : `top: ${r.bottom + 6}px`
    pos = `position: fixed; left: ${left}px; ${vert}; z-index: 70`
    open = true
  }
</script>

<!-- Reposition-on-scroll is guesswork; closing matches DropdownMenu. -->
<svelte:window on:scroll={() => (open = false)} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  bind:this={ref}
  class={cn('group relative inline-flex align-middle', className)}
  onmouseenter={show}
  onmouseleave={() => (open = false)}
>
  <Info size={13} class="cursor-help text-muted transition-colors duration-[120ms] group-hover:text-fg" />
</span>
{#if open}
  <span
    use:portal
    role="tooltip"
    style={pos}
    in:pop={POPOVER}
    out:fade={QUICK}
    class="pointer-events-none w-64 rounded-lg border border-line bg-panel px-2.5 py-2 font-sans text-[11px] font-normal normal-case leading-snug tracking-normal text-muted shadow-[var(--theme-shadow-2)]"
  >
    {text}
  </span>
{/if}

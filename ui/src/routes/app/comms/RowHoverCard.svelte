<script module lang="ts">
  // When the last card closed (performance.now()). A card opened within
  // SKIM_MS of it is part of the same skim: it opens at once, with no
  // entrance.
  let lastClosedAt = -Infinity
</script>

<script lang="ts">
  import type { Snippet } from 'svelte'
  import { portal } from '@/lib/portal'
  import { fade, pop, POPOVER } from '@/lib/motion'

  // A rail row's hover card: rest the pointer on a row and a small card opens
  // beside the rail with what the row is (an agent's role and threads, a
  // thread's agent and age). It PORTALS to <body> and is placed from the row's
  // rect, like InfoTip, so the scrolling rail cannot clip it. The delay keeps
  // it from flashing while the pointer travels down the list.
  let { card, children }: { card: Snippet; children: Snippet } = $props()

  const DELAY = 450
  const WIDTH = 240
  const MARGIN = 8
  const SKIM_MS = 300

  let ref = $state<HTMLDivElement | null>(null)
  let pos = $state<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | null = null
  let instant = $state(false)

  function show() {
    const r = ref?.getBoundingClientRect()
    if (!r) return
    const left = Math.min(r.right + MARGIN, window.innerWidth - WIDTH - MARGIN)
    // Level with the row; pulled up when it would run off the bottom.
    const top = Math.min(r.top, window.innerHeight - 120)
    pos = `position: fixed; left: ${left}px; top: ${top}px; width: ${WIDTH}px; z-index: 60`
  }
  const enter = () => {
    if (timer) clearTimeout(timer)
    if (performance.now() - lastClosedAt < SKIM_MS) {
      instant = true
      show()
      return
    }
    instant = false
    timer = setTimeout(show, DELAY)
  }
  const leave = () => {
    if (timer) clearTimeout(timer)
    timer = null
    if (pos) lastClosedAt = performance.now()
    pos = null
  }
  $effect(() => leave)
</script>

<svelte:window on:scroll|capture={leave} />

<!-- svelte-ignore a11y_no_static_element_interactions -- reason: a pointer-only preview; everything in the card is reachable from the row itself -->
<div bind:this={ref} onmouseenter={enter} onmouseleave={leave} onmousedown={leave}>
  {@render children()}
</div>
{#if pos}
  <div
    use:portal
    role="tooltip"
    style={pos}
    in:pop={instant ? { duration: 0 } : POPOVER}
    out:fade={{ duration: 0 }}
    class="pointer-events-none origin-left rounded-lg border border-line bg-panel px-3 py-2 font-sans text-xs text-muted shadow-[var(--theme-shadow-2)]"
  >
    {@render card()}
  </div>
{/if}

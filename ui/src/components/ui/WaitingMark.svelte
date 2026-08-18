<script lang="ts">
  import { animationById } from '@/lib/waiting/animations'
  import { gridAnimationById } from '@/lib/waiting/grid-animations'
  import { GRID_SCALE } from '@/lib/waiting/scale'
  import { waitingFor, type WaitingSiteRef } from '@/lib/waiting/session.svelte'
  import WaitingBraille from './WaitingBraille.svelte'
  import WaitingGrid from './WaitingGrid.svelte'

  /**
   * The waiting mark: "an agent is working on this right now", as an activity
   * signal beside a label.
   *
   * A call site names its SITE and nothing else. Which of the thirty states it
   * gets, and how fast that state plays, are both decided by the rotation
   * (`lib/waiting/`) from this session's seed — so the cockpit shows a different
   * cast each session while staying internally consistent within one, and no
   * component has an opinion about which mark is "the" mark.
   *
   * In-app that site is a KEY from `sites.ts`, and an unknown one is a type
   * error. Code built against the SDK cannot add rows to that table, so it
   * passes a descriptor instead — `{ key, role }` — and gets a hashed pick
   * rather than a dealt one. Both re-roll on the same session seed.
   *
   * This is `mark` territory, not `Skeleton` and not `Generating`:
   *   Skeleton    a FETCH has not resolved      → signal static
   *   Generating  MODEL OUTPUT is being written → bar rows shaped like the text
   *   WaitingMark an agent is WORKING right now → this
   *
   * `size` is the one thing the call site owns, because only it knows whether
   * the mark sits in an 11px mono row or a 15px sans one. The `slot` in the site
   * table already guaranteed whatever got dealt physically fits.
   */
  let {
    site,
    size = 14,
    class: className,
  }: {
    site: WaitingSiteRef
    /** Font size in px for braille; the grid derives its box from it. */
    size?: number
    class?: string
  } = $props()

  const resolved = $derived(waitingFor(site))
  const braille = $derived(animationById(resolved.state.slug))
  const grid = $derived(gridAnimationById(resolved.state.slug))
</script>

{#if braille}
  <WaitingBraille animation={braille} {size} speed={resolved.speed} class={className} />
{:else if grid}
  <!-- GRID_SCALE, applied HERE and only here. A braille glyph fills most of its
       em; a 5×5 lattice spends 38% of its box on gaps, so matched px sizes make
       the grid read markedly smaller than the text beside it. Every call site
       passing one `size` and getting equal visual weight from either family is
       the whole reason this dispatcher exists rather than two imports. -->
  <WaitingGrid animation={grid} size={size * GRID_SCALE} speed={resolved.speed} class={className} />
{/if}

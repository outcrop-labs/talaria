<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import DitherLayer, { bleedFor, rectIn, spreadFor, type RectShape } from './DitherLayer.svelte'
  import type { DitherSource } from '@/lib/dither'
  import { focusRing } from './control'

  // The compact control for contextual actions — an icon with a tooltip, not a
  // labeled button. Use wherever placement implies the verb (rail headers,
  // surface headers, row hover actions); reserve labeled <Button>s for a page's
  // one or two primary actions and anything destructive/deploying.
  interface Props extends HTMLButtonAttributes {
    /** Tooltip — REQUIRED: the icon carries no label. */
    title: string
    active?: boolean
    danger?: boolean
    size?: 'sm' | 'md'
    /** bind:ref for imperative focus/measure at call sites that need it. */
    ref?: HTMLButtonElement | null
    /** The dither bloom on approach. On by default, as on Button — an icon
     *  tile is a button, and a toolbar where only the labelled controls
     *  respond to the pointer reads as half-finished. */
    bloom?: boolean
  }

  let {
    title,
    active,
    danger,
    size = 'md',
    class: className,
    children,
    ref = $bindable(null),
    bloom = true,
    disabled,
    ...rest
  }: Props = $props()

  const wantsBloom = $derived(bloom && !disabled)

  let wrap = $state<HTMLSpanElement | null>(null)
  let rect = $state<RectShape | null>(null)
  let radius = $state(0)

  /** Proportional to the measured control — see `spreadFor`. */
  const spread = $derived(rect ? spreadFor(Math.min(rect.w, rect.h)) : 0)

  // Same rule as Button — the reach is a fraction of the tile, so a 28px
  // square gets a rim rather than a halo twice its own size.
  const BLEED = bleedFor([spreadFor(9999)])
  // ZERO, deliberately. The field is strictly outside (`inner: 0`), so any pad
  // is a ring of guaranteed emptiness between the border and the first dot —
  // it reads as the treatment floating off the control. The radius is taken
  // raw for the same reason: padding the rect would need padding the corner to
  // match, and both were just pushing the field away.
  const PAD = 0

  const arm = () => {
    if (!wrap || !ref) return
    rect = rectIn(wrap, ref, PAD, BLEED)
    radius = parseFloat(getComputedStyle(ref).borderTopLeftRadius) || 0
  }
  const disarm = () => (rect = null)

  const sources = $derived<DitherSource[]>(
    rect
      ? [
          {
            id: 'bloom',
            kind: 'rect',
            ...rect,
            radius,
            spread,
            strength: 0.95,
            inner: 0,
            rim: 0,
            tone: danger ? 'danger' : 'neutral',
          },
        ]
      : [],
  )
</script>

{#snippet control()}
  <button
    bind:this={ref}
    type="button"
    {title}
    {disabled}
    aria-label={title}
    class={cn(
      // Mercury icon tile (spec §8): radius 6, raised when active,
      // hover-token fill, danger only ever tints the glyph — never a fill.
      // `relative` so the absolutely-positioned field paints BEHIND the tile
      // rather than over its edges — see the note in Button.
      'relative grid shrink-0 place-items-center rounded-md transition-colors disabled:opacity-40',
      focusRing,
      size === 'sm' ? 'h-7 w-7' : 'h-8 w-8',
      active ? 'bg-raised text-fg' : 'text-muted hover:dither-fill',
      danger ? 'hover:text-danger' : 'hover:text-fg',
      className,
    )}
    {...rest}
    onmouseenter={(e) => {
      arm()
      rest.onmouseenter?.(e)
    }}
    onmouseleave={(e) => {
      disarm()
      rest.onmouseleave?.(e)
    }}
    onfocus={(e) => {
      arm()
      rest.onfocus?.(e)
    }}
    onblur={(e) => {
      disarm()
      rest.onblur?.(e)
    }}
  >
    {@render children?.()}
  </button>
{/snippet}

{#if wantsBloom}
  <!-- `inline-flex` shrink-wraps the tile, and the canvas spills past it via
       `bleed`, so the halo costs no layout. -->
  <span bind:this={wrap} class="relative inline-flex shrink-0">
    <!-- FINER GRAIN THAN THE DEFAULT, because a band is only a few px wide.
         At the house 4px pitch an 8px band is two rows of dots, which reads as
         a dotted outline; at 2px it is four rows of half-size dots and reads as
         grain hugging the edge. Large fields (the empty-state vignette, the
         skeleton static) keep the coarser pitch — they have room for it, and
         finer dots over a whole pane is a lot of fill for no more information.

         A 2px lattice is a subdivision of the 4px one, so this stays in phase
         with every other field on the page rather than starting a second grid. -->
    <DitherLayer {sources} bleed={BLEED} organic={0.25} alphaFloor={0.02} maxAlpha={0.85} />
    {@render control()}
  </span>
{:else}
  {@render control()}
{/if}

<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import DitherLayer, { bleedFor, rectIn, type RectShape } from './DitherLayer.svelte'
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

  // Tighter than Button's: an icon tile is 28-32px square, and the same 22px
  // reach a wide label wears would swamp it.
  const SPREAD = 14
  const BLEED = bleedFor([SPREAD])
  const PAD = 1

  const arm = () => {
    if (!wrap || !ref) return
    rect = rectIn(wrap, ref, PAD, BLEED)
    radius = (parseFloat(getComputedStyle(ref).borderTopLeftRadius) || 0) + PAD
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
            spread: SPREAD,
            strength: 0.6,
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
      'grid shrink-0 place-items-center rounded-md transition-colors disabled:opacity-40',
      focusRing,
      size === 'sm' ? 'h-7 w-7' : 'h-8 w-8',
      active ? 'bg-raised text-fg' : 'text-muted hover:bg-hover',
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
    <DitherLayer {sources} bleed={BLEED} organic={0.35} />
    {@render control()}
  </span>
{:else}
  {@render control()}
{/if}

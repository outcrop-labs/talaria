<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { cn } from '@/lib/cn'
  import { useField } from '@/lib/field-registry.svelte'
  import { FIELD_BAND } from '@/lib/field-effects'
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

  let hot = $state(false)
  let radius = $state(6)

  // Measured on APPROACH, as on Button: the radius is the one thing about the
  // control the surface cannot read for itself from a bounding box.
  const arm = () => {
    if (ref) radius = parseFloat(getComputedStyle(ref).borderTopLeftRadius) || 0
    hot = true
  }
  const disarm = () => (hot = false)

  // THE WRAPPER IS GONE, and with it `rectIn`, `bleedFor` and the bled canvas.
  // The field is drawn by the surrounding FieldSurface from this element's own
  // box, read fresh each draw, so there is nothing to measure against and
  // nothing to keep in sync. A tile that used to need a `relative inline-flex`
  // span around it is now just the tile — which also means a call site's
  // layout classes land on the control instead of on a wrapper.
  //
  // `inner: 0, rim: 0` keeps the interior clean: dots behind a glyph muddy it.
  useField(
    () => ref,
    (): DitherSource[] =>
      wantsBloom && hot
        ? [
            {
              id: 'bloom',
              kind: 'rect',
              x: 0,
              y: 0,
              w: 0,
              h: 0,
              radius,
              spread: FIELD_BAND,
              strength: 0.95,
              inner: 0,
              rim: 0,
              falloff: 2,
              tone: danger ? 'danger' : 'neutral',
            },
          ]
        : [],
  )
</script>

<button
  data-dither-fill
  bind:this={ref}
  type="button"
  {title}
  {disabled}
  aria-label={title}
  class={cn(
    // Mercury icon tile (spec §8): radius 6, raised when active, hover-token
    // fill, danger only ever tints the glyph — never a fill.
    'grid shrink-0 place-items-center rounded-md transition-colors disabled:opacity-40',
    focusRing,
    size === 'sm' ? 'h-7 w-7' : 'h-8 w-8',
    active ? 'bg-raised text-fg' : 'text-muted',
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

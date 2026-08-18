<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { spreadFor } from './DitherLayer.svelte'
  import { useField } from '@/lib/field-registry.svelte'
  import { buttonClasses, type ButtonSize, type ButtonVariant } from './button'
  import type { DitherSource, DitherTone } from '@/lib/dither'

  interface Props extends HTMLButtonAttributes {
    variant?: ButtonVariant
    size?: ButtonSize
    /** bind:ref for imperative focus/measure at call sites that need it. */
    ref?: HTMLButtonElement | null
    /**
     * A dither bloom on approach — Mercury's matte substitute for glow.
     *
     * ON BY DEFAULT for every variant that has a frame. `link` is the
     * permanent exception: it is prose with no frame, so there is no boundary
     * for a band to hug and a field around a word in a sentence is a smudge.
     * Disabled controls stay quiet too — a dimmed button that still reaches
     * for the pointer is telling two stories.
     *
     * The field is drawn by the surrounding `FieldSurface`, not by this
     * component. Outside one it simply does not appear, which is why there is
     * no cost argument here any more: the button contributes an entry to an
     * array, and the whole surface is one draw call however many contribute.
     */
    bloom?: boolean
  }

  let {
    variant = 'primary',
    size = 'md',
    type = 'button',
    class: className,
    children,
    ref = $bindable(null),
    bloom,
    disabled,
    ...rest
  }: Props = $props()

  // The field carries the control's own meaning. A destructive action reaching
  // for you in gold would be the wrong promise.
  const TONES: Partial<Record<ButtonVariant, DitherTone>> = {
    primary: 'accent',
    'accent-soft': 'accent',
    danger: 'danger',
    'danger-outline': 'danger',
  }

  const wantsBloom = $derived((bloom ?? variant !== 'link') && !disabled)

  let hot = $state(false)
  let radius = $state(6)

  // Measured on APPROACH rather than at mount: the page reflows constantly, and
  // the radius is the one thing about the control the surface cannot read for
  // itself from a bounding box.
  const arm = () => {
    if (ref) radius = parseFloat(getComputedStyle(ref).borderTopLeftRadius) || 0
    hot = true
  }
  const disarm = () => (hot = false)

  // Coordinates are relative to the control's own box, which the surface reads
  // fresh each draw — so there is no wrapper to measure against, no canvas to
  // size and no bleed to reconcile. `inner: 0, rim: 0` keeps the interior
  // clean: dots behind the label made the transparent variants unreadable.
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
              spread: spreadFor(0),
              strength: 0.95,
              inner: 0,
              rim: 0,
              falloff: 2,
              tone: TONES[variant] ?? 'neutral',
            },
          ]
        : [],
  )
</script>

<!-- The one button. Reuse everywhere — do not re-style buttons inline.
     No wrapper element: the field lives on the surface, so the control is the
     control. `splitLayoutClasses` went with the wrapper — a call site's
     `ml-auto` applies to the button itself again, as it always should have. -->
<button
  bind:this={ref}
  {type}
  {disabled}
  class={buttonClasses({ variant, size, className: className as string | null })}
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

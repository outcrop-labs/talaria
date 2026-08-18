<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import DitherLayer, { bleedFor, rectIn, spreadFor, type RectShape } from './DitherLayer.svelte'
  import { buttonClasses, splitLayoutClasses, type ButtonSize, type ButtonVariant } from './button'
  import type { DitherSource, DitherTone } from '@/lib/dither'

  interface Props extends HTMLButtonAttributes {
    variant?: ButtonVariant
    size?: ButtonSize
    /** bind:ref for imperative focus/measure at call sites that need it. */
    ref?: HTMLButtonElement | null
    /**
     * A dither bloom on approach — Mercury's matte substitute for glow.
     *
     * ON BY DEFAULT FOR EVERY VARIANT THAT HAS A FRAME. It was primary-only at
     * first, on a cost argument that stopped being true the same afternoon:
     * the per-instance MutationObserver and media listener became shared
     * ref-counted subscriptions when the skeleton field needed the same two
     * signals, so the page pays for one of each however many fields exist.
     * What is left per instance is a canvas and an rAF loop the engine parks
     * the moment the field is static — an un-hovered button costs one paint.
     *
     * With the cost gone, primary-only was just an inconsistency: a person
     * reaching for Cancel got nothing back while Deploy bloomed, which reads
     * as the treatment being broken rather than as emphasis.
     *
     * `link` is the exception and always will be: it is prose with no frame,
     * so there is no boundary for a halo to hug — a field around a word in a
     * sentence is a smudge.
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

  // A disabled control must not reach for the pointer — the bloom reads as an
  // invitation, and a dimmed button that still blooms is telling two stories.
  const wantsBloom = $derived((bloom ?? variant !== 'link') && !disabled)

  let wrap = $state<HTMLSpanElement | null>(null)
  let rect = $state<RectShape | null>(null)
  let radius = $state(0)

  /** Proportional to the measured control — see `spreadFor`. */
  const spread = $derived(rect ? spreadFor(Math.min(rect.w, rect.h)) : 0)

  /** The canvas is sized for the LARGEST halo this button could want, because
   *  it is created once and the reach is only known after measuring. A button
   *  is never taller than the md size, so the ceiling of `spreadFor` bounds it. */
  const BLEED = bleedFor([spreadFor(9999)])
  // ZERO, deliberately. The field is strictly outside (`inner: 0`), so any pad
  // is a ring of guaranteed emptiness between the border and the first dot —
  // it reads as the treatment floating off the control. The radius is taken
  // raw for the same reason: padding the rect would need padding the corner to
  // match, and both were just pushing the field away.
  const PAD = 0
  /** Proportional to the control: a fixed reach makes a small button wear a
   *  cloud and lets neighbours in a toolbar merge into one. */

  // Measured on APPROACH, not on mount: the page reflows constantly and a rect
  // cached at mount is wrong by the time anyone hovers it.
  const arm = () => {
    if (!wrap || !ref) return
    rect = rectIn(wrap, ref, PAD, BLEED)
    radius = parseFloat(getComputedStyle(ref).borderTopLeftRadius) || 0
  }
  const disarm = () => (rect = null)

  // `inner: 0, rim: 0` keeps the interior CLEAN. Dots behind the label made the
  // transparent variants unreadable, so the bloom is strictly outside — and it
  // takes the button's own radius so the corners stay corners rather than
  // squaring off under the densest dots.
  // The wrapper becomes the flex child, so anything positioning the button
  // inside its parent has to travel with it.
  const split = $derived(splitLayoutClasses(className as string | null))

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
            tone: TONES[variant] ?? 'neutral',
          },
        ]
      : [],
  )
</script>

<!-- The one button. Reuse everywhere — do not re-style buttons inline. -->
{#if wantsBloom}
  <span bind:this={wrap} class={`relative inline-flex ${split.outer}`}>
    <!-- FINER GRAIN THAN THE DEFAULT, because a band is only a few px wide.
         At the house 4px pitch an 8px band is two rows of dots, which reads as
         a dotted outline; at 2px it is four rows of half-size dots and reads as
         grain hugging the edge. Large fields (the empty-state vignette, the
         skeleton static) keep the coarser pitch — they have room for it, and
         finer dots over a whole pane is a lot of fill for no more information.

         The 2px lattice keeps a FIXED relationship to the 4px one the large
         fields use, so neither drifts against the other and there is no moiré.
         Not literal nesting, though: cell BOUNDARIES nest (page multiples of 2
         contain page multiples of 4) but the DOTS do not. Both engines place a
         dot at `cx*pitch - frac + (pitch-dot)/2`, so a 4/2 dot centres on an
         even page coordinate and a 2/1 dot on an odd one — they interleave by
         half the fine pitch and never coincide. Fixed everywhere, which is the
         property page-anchoring was for; "subdivision" would invite someone to
         expect dots to land on dots.

         `alphaFloor` near zero is what makes the band TAPER. The engine lights
         a dot at `alphaFloor + (maxAlpha - alphaFloor) * density`, so the
         default floor of 0.18 lit even the sparsest outermost dot at a
         perfectly visible level — the field thinned out with distance but
         never dimmed, which is what made a wide band read as a cloud with an
         edge. At 0.02 the outer dots approach nothing while `maxAlpha` 0.85
         keeps the boundary strong: dense and bright against the border,
         dissolving as it leaves. -->
    <DitherLayer {sources} bleed={BLEED} organic={0.15} pitch={2} dot={1} alphaFloor={0.02} maxAlpha={0.85} />
    <!-- The bloom's handlers come AFTER {...rest} and CALL the caller's, so a
         call site that wants its own hover behaviour does not silently replace
         the bloom's — both run. -->
    <button
      bind:this={ref}
      {type}
      {disabled}
      class={buttonClasses({ variant, size, className: split.inner || null })}
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
  </span>
{:else}
  <button
    bind:this={ref}
    {type}
    {disabled}
    class={buttonClasses({ variant, size, className: className as string | null })}
    {...rest}
  >
    {@render children?.()}
  </button>
{/if}

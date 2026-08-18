<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import DitherLayer, { bleedFor, rectIn, type RectShape } from './DitherLayer.svelte'
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

  /** The halo lives outside the control, so the canvas has to reach past it —
   *  `bleed` spills without the wrapper taking any space (see DitherLayer).
   *  Derived from the spread: a smaller bleed cuts the halo off square. */
  const SPREAD = 22
  const BLEED = bleedFor([SPREAD])
  const PAD = 1

  // Measured on APPROACH, not on mount: the page reflows constantly and a rect
  // cached at mount is wrong by the time anyone hovers it.
  const arm = () => {
    if (!wrap || !ref) return
    rect = rectIn(wrap, ref, PAD, BLEED)
    radius = (parseFloat(getComputedStyle(ref).borderTopLeftRadius) || 0) + PAD
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
            spread: SPREAD,
            strength: 0.7,
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
    <DitherLayer {sources} bleed={BLEED} organic={0.35} />
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

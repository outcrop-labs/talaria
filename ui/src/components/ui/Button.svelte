<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { buttonClasses, type ButtonSize, type ButtonVariant } from './button'

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

  // A disabled control must not reach for the pointer — the bloom reads as an
  // invitation, and a dimmed button that still blooms is telling two stories.
  const wantsBloom = $derived((bloom ?? variant !== 'link') && !disabled)

  /**
   * THE FIELD IS PAINTED, NOT MEASURED. It used to be a bled canvas in a
   * wrapper span, with the button's rect measured into it on approach and a
   * halo drawn strictly outside — which needed a wrapper, a bleed, a rect and
   * a re-measure on every hover, and `splitLayoutClasses` to move a call
   * site's layout classes onto the wrapper so the control still sat where it
   * was told.
   *
   * All of it is gone. The `dither-fill` marker is the whole declaration now:
   * `lib/dither-surface.ts` hands the button a field and paints it per cell,
   * the same way the rail, the tabs and every marked row get theirs. No
   * wrapper, so a call site's classes land on the button again.
   */
</script>

<!-- The one button. Reuse everywhere — do not re-style buttons inline. -->
<button
  bind:this={ref}
  {type}
  {disabled}
  class={buttonClasses({
    variant,
    size,
    className: [wantsBloom ? 'dither-fill' : '', className as string | null].filter(Boolean).join(' '),
  })}
  {...rest}
>
  {@render children?.()}
</button>

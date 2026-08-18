import { cn } from '@/lib/cn'
import { controlSizes, focusRing, type ControlSize } from './control'

export type ButtonVariant = 'primary' | 'outline' | 'ghost' | 'danger' | 'danger-outline' | 'accent-soft' | 'link'

const base = cn(
  // whitespace-nowrap: a button label NEVER line-breaks — if space is tight the
  // layout should shrink something else, not fold the label.
  // Mercury (spec §8): buttons speak in the mono chrome voice — uppercase,
  // letterspaced, radius 6 — with a solid gold focus ring. Matte: no glows.
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md font-mono font-medium uppercase tracking-[0.05em] transition-all disabled:pointer-events-none',
  focusRing,
)

// Disabled reading for the non-fill variants: dim the whole control. Text-on-
// transparent/raised stays legible at half opacity in both themes. The primary
// gold fill must NOT use this — dimming the fill and its surface-toned label
// together leaves a ~1.5:1 washed-gold/near-white pair in light mode.
const dimDisabled = 'disabled:opacity-50'

const variants: Record<ButtonVariant, string> = {
  // Primary: warm gold fill, ground-dark label (spec §8 `NEW MESSAGE +`).
  // Disabled swaps to the spec's raised-tile + muted-glyph treatment (§7 send
  // button) — same in both themes, label always survives the dimmed fill.
  primary:
    'bg-accent text-surface hover:brightness-110 disabled:border disabled:border-line disabled:bg-raised disabled:text-muted',
  // Secondary: raised tile + hairline + readout mono label.
  outline: cn('border border-line bg-raised text-fg hover:bg-hover', dimDisabled),
  // Ghost: mono uppercase muted → readout.
  ghost: cn('text-muted hover:text-fg', dimDisabled),
  // Destructive: ORANGE OUTLINE — danger border + danger mono text, never an
  // orange fill (spec §8 / `8A-0` DANGER ZONE).
  danger: cn('border border-danger bg-transparent text-danger hover:bg-danger/10', dimDisabled),
  // Quiet destructive — for danger actions inside dense panels.
  'danger-outline': cn('border border-danger/40 bg-transparent text-danger hover:border-danger', dimDisabled),
  // Soft accent call-to-action (connect/enable nudges) — visible, not primary.
  'accent-soft': cn('border border-accent/60 bg-accent/10 text-accent hover:bg-accent/15', dimDisabled),
  // Text-link action; sizes collapse (no height) — inline with prose, so it
  // keeps the sans reading voice (no mono/uppercase chrome).
  link: cn(
    'font-sans normal-case tracking-normal text-sm text-accent underline-offset-2 hover:underline !h-auto !px-0',
    dimDisabled,
  ),
}

const sizes: Record<ControlSize, string> = {
  sm: `${controlSizes.sm} px-3 text-[11px]`,
  md: `${controlSizes.md} px-4 text-[11px]`,
}

/** Shared button styling — reuse this on <a>/<label> etc. so links match buttons
 *  without duplicating the classes (see the Google sign-in anchor). */
export function buttonClasses(opts?: { variant?: ButtonVariant; size?: ControlSize; className?: string | null }): string {
  const { variant = 'primary', size = 'md', className } = opts ?? {}
  // Variant AFTER size so `link` can restore the sans prose voice.
  return cn(base, sizes[size], variants[variant], className)
}

/**
 * Split a call site's classes into the ones that position the control inside
 * its PARENT and the ones that style the control itself.
 *
 * A bloomed button is wrapped in a span so the halo canvas has something to
 * anchor to, and that wrapper becomes the flex/grid child. `ml-auto` on the
 * inner button then aligns nothing, because the button is alone inside a
 * shrink-wrapped span — the class is still applied, still valid, and silently
 * does nothing. That is the worst kind of regression: no error, just a control
 * that stopped sitting where it used to.
 *
 * So the layout half moves to the wrapper and the rest stays on the button.
 * The list is deliberately an ALLOWLIST of properties that only mean anything
 * relative to a parent — margins, self-alignment, flex-item sizing, order,
 * width. Padding, colour and typography are the control's own business and
 * must not move, or the wrapper grows and pushes the button off its baseline.
 */
export function splitLayoutClasses(className?: string | null): { outer: string; inner: string } {
  if (!className) return { outer: '', inner: '' }
  const outer: string[] = []
  const inner: string[] = []
  for (const cls of className.split(/\s+/).filter(Boolean)) {
    // Strip any variant prefixes (sm:, hover:, dark:) before matching so
    // `sm:ml-auto` is recognised as the margin it is.
    const bare = cls.slice(cls.lastIndexOf(':') + 1)
    ;(LAYOUT_CLASS.test(bare) ? outer : inner).push(cls)
  }
  return { outer: outer.join(' '), inner: inner.join(' ') }
}

const LAYOUT_CLASS =
  /^-?(m[trblxy]?-|self-|justify-self-|order-|basis-|grow(-|$)|shrink(-|$)|flex-(1|auto|initial|none)$|w-(full|auto|px|\d|\[)|col-span-|row-span-)/

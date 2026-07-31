import { forwardRef } from 'react'
import { cn } from '@/lib/cn'
import { controlSizes, focusRing, type ControlSize } from './control'

type Variant = 'primary' | 'outline' | 'ghost' | 'danger' | 'danger-outline' | 'accent-soft' | 'link'
type Size = ControlSize

const base = cn(
  // whitespace-nowrap: a button label NEVER line-breaks — if space is tight the
  // layout should shrink something else, not fold the label.
  // Gentle dew (spec §8): buttons speak in the mono chrome voice — uppercase,
  // letterspaced, radius 6 — with a solid gold focus ring. Matte: no glows.
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md font-mono font-medium uppercase tracking-[0.05em] transition-all disabled:opacity-50 disabled:pointer-events-none',
  focusRing,
)

const variants: Record<Variant, string> = {
  // Primary: warm gold fill, ground-dark label (spec §8 `NEW MESSAGE +`).
  primary: 'bg-accent text-surface hover:brightness-110',
  // Secondary: raised tile + hairline + readout mono label.
  outline: 'border border-line bg-raised text-fg hover:bg-hover',
  // Ghost: mono uppercase muted → readout.
  ghost: 'text-muted hover:text-fg',
  // Destructive: ORANGE OUTLINE — danger border + danger mono text, never an
  // orange fill (spec §8 / `8A-0` DANGER ZONE).
  danger: 'border border-danger bg-transparent text-danger hover:bg-danger/10',
  // Quiet destructive — for danger actions inside dense panels.
  'danger-outline': 'border border-danger/40 bg-transparent text-danger hover:border-danger',
  // Soft accent call-to-action (connect/enable nudges) — visible, not primary.
  'accent-soft': 'border border-accent/60 bg-accent/10 text-accent hover:bg-accent/15',
  // Text-link action; sizes collapse (no height) — inline with prose, so it
  // keeps the sans reading voice (no mono/uppercase chrome).
  link: 'font-sans normal-case tracking-normal text-sm text-accent underline-offset-2 hover:underline !h-auto !px-0',
}

const sizes: Record<Size, string> = {
  sm: `${controlSizes.sm} px-3 text-[11px]`,
  md: `${controlSizes.md} px-4 text-[11px]`,
}

/** Shared button styling — reuse this on <a>/<label> etc. so links match buttons
 *  without duplicating the classes (see the Google sign-in anchor). */
export function buttonClasses(opts?: { variant?: Variant; size?: Size; className?: string }): string {
  const { variant = 'primary', size = 'md', className } = opts ?? {}
  // Variant AFTER size so `link` can restore the sans prose voice.
  return cn(base, sizes[size], variants[variant], className)
}

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: Size
}

/** The one button. Reuse everywhere — do not re-style buttons inline. */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'md', type = 'button', ...props }, ref) => (
    <button ref={ref} type={type} className={buttonClasses({ variant, size, className })} {...props} />
  ),
)
Button.displayName = 'Button'

import { forwardRef } from 'react'
import { cn } from '@/lib/cn'
import { controlSizes, type ControlSize } from './control'

type Variant = 'primary' | 'outline' | 'ghost' | 'danger' | 'danger-outline' | 'accent-soft' | 'link'
type Size = ControlSize

const base =
  // whitespace-nowrap: a button label NEVER line-breaks — if space is tight the
  // layout should shrink something else, not fold the label.
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-xl font-medium transition-all disabled:opacity-50 disabled:pointer-events-none focus-visible:outline-2 focus-visible:outline-offset-2'

const variants: Record<Variant, string> = {
  // Mercury bronze — solid metal, near-black label, matte (no glow).
  primary: 'bg-accent text-surface hover:brightness-110 shadow-[var(--theme-shadow-1)]',
  outline: 'border border-line bg-card text-fg hover:border-[var(--theme-accent-border)]',
  ghost: 'text-muted hover:text-accent',
  danger: 'bg-[color:var(--theme-danger)] text-white hover:brightness-110 shadow-[var(--theme-shadow-1)]',
  // Quiet destructive with a border — for danger actions inside dense panels.
  'danger-outline': 'border border-[color:var(--theme-danger)]/40 bg-card text-[color:var(--theme-danger)] hover:border-[color:var(--theme-danger)]',
  // Soft accent call-to-action (connect/enable nudges) — visible, not primary.
  'accent-soft': 'border border-accent/60 bg-accent/10 text-accent hover:bg-accent/15',
  // Text-link action; sizes collapse (no height) — inline with prose.
  link: 'text-accent underline-offset-2 hover:underline !h-auto !px-0',
}

const sizes: Record<Size, string> = {
  sm: `${controlSizes.sm} px-3`,
  md: `${controlSizes.md} px-4`,
}

/** Shared button styling — reuse this on <a>/<label> etc. so links match buttons
 *  without duplicating the classes (see the Google sign-in anchor). */
export function buttonClasses(opts?: { variant?: Variant; size?: Size; className?: string }): string {
  const { variant = 'primary', size = 'md', className } = opts ?? {}
  return cn(base, variants[variant], sizes[size], className)
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

import { forwardRef } from 'react'
import { cn } from '@/lib/cn'
import { controlSizes, type ControlSize } from './control'

export interface InputProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, 'size'> {
  size?: ControlSize
}

/** The one text input. Reuse everywhere — do not re-style inputs inline. */
export const Input = forwardRef<HTMLInputElement, InputProps>(({ className, size = 'md', ...props }, ref) => (
  <input
    ref={ref}
    className={cn(
      controlSizes[size],
      // What users TYPE is content, not chrome — sans. Callers needing mono
      // (keys, slugs, config) pass font-[var(--font-mono)]; cn() resolves it.
      // Spec §8: raised tile bg, hairline border, radius 6, gold focus ring.
      'w-full rounded-md border border-line bg-[var(--theme-input)] px-3 font-sans text-sm text-fg outline-none transition-colors',
      'placeholder:text-muted focus:border-accent focus:ring-2 focus:ring-accent-soft',
      className,
    )}
    {...props}
  />
))
Input.displayName = 'Input'

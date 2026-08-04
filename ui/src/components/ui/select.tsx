import { forwardRef } from 'react'
import { cn } from '@/lib/cn'
import { controlSizes, type ControlSize } from './control'

export interface SelectProps extends Omit<React.SelectHTMLAttributes<HTMLSelectElement>, 'size'> {
  size?: ControlSize
}

/** The one select. Reuse for status/priority/agent/role controls. */
export const Select = forwardRef<HTMLSelectElement, SelectProps>(
  ({ className, size = 'md', children, ...props }, ref) => (
    <select
      ref={ref}
      className={cn(
        controlSizes[size],
        // Spec §8: raised tile bg, hairline border, radius 6, gold focus ring.
        'rounded-md border border-line bg-[var(--theme-input)] px-2.5 font-sans text-sm text-fg outline-none transition-colors',
        'focus:border-accent focus:ring-2 focus:ring-accent-soft',
        className,
      )}
      {...props}
    >
      {children}
    </select>
  ),
)
Select.displayName = 'Select'

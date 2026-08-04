// The compact control for contextual actions — an icon with a tooltip, not a
// labeled button. Use wherever placement implies the verb (rail headers,
// surface headers, row hover actions); reserve labeled <Button>s for a page's
// one or two primary actions and anything destructive/deploying.
import { forwardRef } from 'react'
import { cn } from '@/lib/cn'
import { focusRing } from './control'

export const IconButton = forwardRef<
  HTMLButtonElement,
  React.ButtonHTMLAttributes<HTMLButtonElement> & {
    /** Tooltip — REQUIRED: the icon carries no label. */
    title: string
    active?: boolean
    danger?: boolean
    size?: 'sm' | 'md'
  }
>(function IconButton({ title, active, danger, size = 'md', className, ...rest }, ref) {
  return (
    <button
      ref={ref}
      type="button"
      title={title}
      aria-label={title}
      className={cn(
        // Gentle dew icon tile (spec §8): radius 6, raised when active,
        // hover-token fill, danger only ever tints the glyph — never a fill.
        'grid shrink-0 place-items-center rounded-md transition-colors disabled:opacity-40',
        focusRing,
        size === 'sm' ? 'h-7 w-7' : 'h-8 w-8',
        active ? 'bg-raised text-fg' : 'text-muted hover:bg-hover',
        danger ? 'hover:text-danger' : 'hover:text-fg',
        className,
      )}
      {...rest}
    />
  )
})

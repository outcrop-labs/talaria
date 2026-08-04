import type { ReactNode } from 'react'
import { cn } from '@/lib/cn'

// The one empty/zero state. Centered mark + short line + optional single
// action. Reuse for every no-data view — don't hand-roll bare "No X yet"
// strings. `compact` for zero states inside panels/lists (no full-height
// centering, smaller mark); `inline` for a single quiet line.
export function EmptyState({
  icon = '◇',
  title,
  hint,
  action,
  variant = 'full',
  className,
}: {
  icon?: ReactNode
  title: string
  hint?: string
  action?: ReactNode
  variant?: 'full' | 'compact' | 'inline'
  className?: string
}) {
  // Spec §2: empty-state copy is reading voice — sans, never the mono chrome
  // voice the app shell inherits from the base font.
  if (variant === 'inline') {
    return (
      <div className={cn('font-sans text-xs text-muted', className)}>
        {title}
        {hint ? ` — ${hint}` : ''}
      </div>
    )
  }
  return (
    <div className={cn(variant === 'full' ? 'grid h-full place-items-center p-6' : 'px-2 py-6', 'text-center', className)}>
      <div className={cn('max-w-xs', variant === 'compact' && 'mx-auto')}>
        <div className={cn('mx-auto text-ink-dim', variant === 'full' ? 'mb-3 text-3xl' : 'mb-2 text-xl')}>{icon}</div>
        <div className={cn('font-sans font-medium text-fg', variant === 'full' ? 'text-sm' : 'text-xs')}>{title}</div>
        {hint && <div className="mt-1 font-sans text-xs leading-5 text-muted">{hint}</div>}
        {action && <div className={variant === 'full' ? 'mt-4' : 'mt-3'}>{action}</div>}
      </div>
    </div>
  )
}

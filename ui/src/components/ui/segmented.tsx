// The one segmented control — a bordered group of exclusive options. Use for
// small mode switches (read/edit, list/grid); Tabs for page-level sections.
import { cn } from '@/lib/cn'

export interface SegmentedOption<T extends string = string> {
  id: T
  label: React.ReactNode
  title?: string
}

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  size = 'sm',
  className,
}: {
  options: ReadonlyArray<SegmentedOption<T>>
  value: T
  onChange: (id: T) => void
  size?: 'xs' | 'sm'
  className?: string
}) {
  return (
    <div className={cn('inline-flex shrink-0 rounded-md border border-line p-0.5', className)}>
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          title={o.title}
          onClick={() => onChange(o.id)}
          className={cn(
            'rounded transition-colors',
            size === 'xs' ? 'px-2 py-0.5 text-[11px]' : 'px-2.5 py-1 text-xs',
            value === o.id ? 'bg-card text-fg' : 'text-muted hover:text-fg',
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

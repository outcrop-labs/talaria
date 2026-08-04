// The one segmented control — a bordered group of exclusive options. Use for
// small mode switches (read/edit, list/grid); Tabs for page-level sections.
// Gentle dew: the AUTO/MANUAL mono toggle from board `8A-0` — hairline group,
// active cell raised + readout, inactive cells muted, mono uppercase labels.
import { cn } from '@/lib/cn'
import { focusRing } from './control'

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
          aria-pressed={value === o.id}
          onClick={() => onChange(o.id)}
          className={cn(
            'rounded font-mono uppercase tracking-[0.05em] transition-colors',
            focusRing,
            size === 'xs' ? 'px-2 py-0.5 text-[10px] leading-3' : 'px-2.5 py-1 text-[10px] leading-4',
            value === o.id ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

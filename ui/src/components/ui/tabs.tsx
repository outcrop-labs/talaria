// The one tab strip — underline style, accent marker on the active tab.
// Every tabbed surface uses this; nobody re-rolls the border-b + marker span.
import { cn } from '@/lib/cn'

export interface TabItem<T extends string = string> {
  id: T
  label: React.ReactNode
}

export function Tabs<T extends string>({
  items,
  value,
  onChange,
  className,
}: {
  items: ReadonlyArray<TabItem<T>>
  value: T
  onChange: (id: T) => void
  className?: string
}) {
  return (
    <div className={cn('flex gap-1 border-b border-line-subtle', className)}>
      {items.map((t) => (
        <button
          key={t.id}
          type="button"
          onClick={() => onChange(t.id)}
          className={cn('relative px-3 py-2 text-sm transition-colors', value === t.id ? 'text-fg' : 'text-muted hover:text-fg')}
        >
          {t.label}
          {value === t.id && <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />}
        </button>
      ))}
    </div>
  )
}

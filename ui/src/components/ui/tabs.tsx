// The one tab strip — Gentle-dew tile tabs (boards `44Y-0`/`8A-0`): mono
// uppercase labels, the active tab a raised hairline tile, dashed-gold
// keyboard focus. Every tabbed surface uses this; nobody re-rolls it.
import { cn } from '@/lib/cn'
import { focusGold } from '@/components/chat/chat-chrome'

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
    <div className={cn('flex items-center gap-1', className)}>
      {items.map((t) => (
        <button
          key={t.id}
          type="button"
          onClick={() => onChange(t.id)}
          className={cn(
            'flex h-7 items-center rounded-md border px-2.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
            focusGold,
            value === t.id
              ? 'border-line bg-raised text-fg'
              : 'border-transparent text-muted hover:bg-hover hover:text-fg',
          )}
        >
          {t.label}
        </button>
      ))}
    </div>
  )
}

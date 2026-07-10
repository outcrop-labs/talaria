// The composer's compact choice pill — the TierPicker pattern, generalized:
// an icon + current value that opens a portaled popover. Use for anything a
// composer needs decided at send time (model tier, research depth, acting
// agent) instead of parking wide controls in rails or headers.
import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { ChevronDown, Check, type LucideIcon } from 'lucide-react'
import { cn } from '@/lib/cn'

export interface ComposerOption {
  value: string
  label: string
  /** Secondary line in the popover (tagline, role, timing). */
  sub?: string
}

export function ComposerPicker({
  icon: Icon,
  value,
  options,
  onChange,
  title,
  menuLabel,
  className,
}: {
  icon: LucideIcon
  value: string
  options: ComposerOption[]
  onChange: (v: string) => void
  title: string
  menuLabel: string
  className?: string
}) {
  const [open, setOpen] = useState(false)
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null)
  const btnRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    if (!open) return
    const place = () => {
      const r = btnRef.current?.getBoundingClientRect()
      if (r) setPos({ left: r.left, bottom: window.innerHeight - r.top + 6 })
    }
    place()
    const onDoc = (e: MouseEvent) => {
      if (!btnRef.current?.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    document.addEventListener('mousedown', onDoc)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      document.removeEventListener('mousedown', onDoc)
    }
  }, [open])

  const current = options.find((o) => o.value === value)
  return (
    <>
      <button
        ref={btnRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        className={cn(
          'flex shrink-0 items-center gap-1.5 self-end mb-[7px] rounded-full border border-line-subtle px-2.5 py-1.5 text-xs text-muted transition-colors hover:border-accent hover:text-fg',
          className,
        )}
        title={title}
      >
        <Icon size={13} />
        <span className="max-w-28 truncate">{current?.label ?? value}</span>
        <ChevronDown size={12} />
      </button>
      {open &&
        pos &&
        typeof document !== 'undefined' &&
        createPortal(
          <div
            className="mercury-panel fixed z-[60] min-w-48 overflow-hidden rounded-xl p-1"
            style={{ left: pos.left, bottom: pos.bottom }}
          >
            <div className="px-2 py-1 text-[10px] uppercase tracking-wide text-muted">{menuLabel}</div>
            {options.map((o) => (
              <button
                key={o.value}
                type="button"
                onClick={() => {
                  onChange(o.value)
                  setOpen(false)
                }}
                className={cn(
                  'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
                  o.value === value ? 'text-fg' : 'text-muted',
                )}
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{o.label}</span>
                  {o.sub && <span className="block truncate text-[10px] text-muted">{o.sub}</span>}
                </span>
                {o.value === value && <Check size={13} className="shrink-0 text-accent" />}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  )
}

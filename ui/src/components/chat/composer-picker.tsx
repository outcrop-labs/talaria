// The composer's compact choice chip — the TierPicker pattern, generalized:
// an icon + current value that opens a portaled popover. Use for anything a
// composer needs decided at send time (model tier, research depth, acting
// agent) instead of parking wide controls in rails or headers.
//
// Gentle dew (spec §7): a secondary 36px mono chip (hairline border, muted →
// readout on hover) over the §7 popover pattern — search row with ⌘K hint,
// panel bg, mono section header, right-aligned mono meta, hover fill,
// dashed-gold selected row.
import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { type LucideIcon } from 'lucide-react'
import { cn } from '@/lib/cn'
import { PopSearch, chipSecondary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'

export interface ComposerOption {
  value: string
  label: string
  /** Right-aligned mono meta in the popover (tagline, role, timing). */
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
  const [q, setQ] = useState('')
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const place = () => {
      const r = btnRef.current?.getBoundingClientRect()
      if (r) setPos({ left: r.left, bottom: window.innerHeight - r.top + 6 })
    }
    place()
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node
      if (!btnRef.current?.contains(t) && !panelRef.current?.contains(t)) setOpen(false)
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
  const needle = q.trim().toLowerCase()
  const visible = options.filter(
    (o) => !needle || o.label.toLowerCase().includes(needle) || o.sub?.toLowerCase().includes(needle),
  )
  return (
    <>
      <button
        ref={btnRef}
        type="button"
        onClick={() => {
          setQ('')
          setOpen((v) => !v)
        }}
        className={cn(chipSecondary, className)}
        title={title}
      >
        <Icon size={12} />
        <span className="max-w-28 truncate">{current?.label ?? value}</span>
      </button>
      {open &&
        pos &&
        typeof document !== 'undefined' &&
        createPortal(
          <div
            ref={panelRef}
            className={cn(popPanel, 'fixed z-[60] min-w-56 overflow-hidden')}
            style={{ left: pos.left, bottom: pos.bottom }}
          >
            <PopSearch value={q} onChange={setQ} placeholder={`Search ${menuLabel.toLowerCase()}`} />
            <div className={popHeader}>{menuLabel}</div>
            {visible.map((o) => (
              <button
                key={o.value}
                type="button"
                onClick={() => {
                  onChange(o.value)
                  setOpen(false)
                }}
                className={cn(popRow, o.value === value ? popRowSelected : 'text-muted')}
              >
                <span className="min-w-0 flex-1 truncate">{o.label}</span>
                {o.sub && (
                  <span className="max-w-44 shrink-0 truncate text-right font-mono text-[10px] tracking-[0.05em] text-ink-dim">
                    {o.sub}
                  </span>
                )}
              </button>
            ))}
            {visible.length === 0 && <div className="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>}
          </div>,
          document.body,
        )}
    </>
  )
}

import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/cn'
import { MeterBars, PopSearch, chipPrimary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'

// The composer's model chip (Mercury spec §7): a 36px mono chip — ✳ glyph,
// tier name, and a 3×12 meter showing where the pick sits on the agent's tier
// ladder — that opens the §7 popover (search row with ⌘K hint, panel bg, mono
// section header, hover fill, dashed-gold selected row). Beats the raw
// <select> — keyboard-navigable and portaled so it escapes the composer's
// clipping.
export function TierPicker({ tiers, value, onChange }: { tiers: string[]; value: string; onChange: (t: string) => void }) {
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState('')
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const options = ['', ...tiers] // '' = the agent's main model

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

  const selectedIndex = Math.max(0, options.indexOf(value))
  const label = value || 'main'
  const needle = q.trim().toLowerCase()
  const visible = options.filter((t) => (t || 'main model').toLowerCase().includes(needle))
  return (
    <>
      <button
        ref={btnRef}
        type="button"
        onClick={() => {
          setQ('')
          setOpen((v) => !v)
        }}
        className={chipPrimary}
        title="Model tier for this chat"
      >
        <span aria-hidden className="text-[10px] leading-none">✳</span>
        <span className="max-w-24 truncate">{label}</span>
        <MeterBars total={options.length} lit={selectedIndex + 1} />
      </button>
      {open &&
        pos &&
        typeof document !== 'undefined' &&
        createPortal(
          <div
            ref={panelRef}
            className={cn(popPanel, 'fixed z-[60] min-w-44 overflow-hidden')}
            style={{ left: pos.left, bottom: pos.bottom }}
          >
            <PopSearch value={q} onChange={setQ} placeholder="Search tiers" />
            <div className={popHeader}>Model tier</div>
            {visible.map((t) => (
              <button
                key={t || 'main'}
                type="button"
                onClick={() => {
                  onChange(t)
                  setOpen(false)
                }}
                className={cn(popRow, t === value ? popRowSelected : 'text-muted')}
              >
                <span className="min-w-0 flex-1 truncate">{t || 'main model'}</span>
                <MeterBars total={options.length} lit={options.indexOf(t) + 1} className="shrink-0" />
              </button>
            ))}
            {visible.length === 0 && <div className="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>}
          </div>,
          document.body,
        )}
    </>
  )
}

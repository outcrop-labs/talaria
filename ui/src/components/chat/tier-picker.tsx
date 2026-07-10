import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Cpu, ChevronDown, Check } from 'lucide-react'
import { cn } from '@/lib/cn'

// A compact model-tier picker for the composer: a pill showing the active tier
// that opens a clean popover. Beats the raw <select> — keyboard-navigable,
// portaled (so it escapes the composer's clipping), and it reads like a choice
// of "brains" rather than a form field.
export function TierPicker({ tiers, value, onChange }: { tiers: string[]; value: string; onChange: (t: string) => void }) {
  const [open, setOpen] = useState(false)
  const [pos, setPos] = useState<{ left: number; bottom: number } | null>(null)
  const btnRef = useRef<HTMLButtonElement>(null)
  const options = ['', ...tiers] // '' = the agent's main model

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

  const label = value || 'main'
  return (
    <>
      <button
        ref={btnRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex shrink-0 items-center gap-1.5 self-end mb-[7px] rounded-full border border-line-subtle px-2.5 py-1.5 text-xs text-muted transition-colors hover:border-accent hover:text-fg"
        title="Model tier for this chat"
      >
        <Cpu size={13} />
        <span className="max-w-24 truncate">{label}</span>
        <ChevronDown size={12} />
      </button>
      {open &&
        pos &&
        typeof document !== 'undefined' &&
        createPortal(
          <div
            className="mercury-panel fixed z-[60] min-w-40 overflow-hidden rounded-xl p-1"
            style={{ left: pos.left, bottom: pos.bottom }}
          >
            <div className="px-2 py-1 text-[10px] uppercase tracking-wide text-muted">Model tier</div>
            {options.map((t) => (
              <button
                key={t || 'main'}
                type="button"
                onClick={() => {
                  onChange(t)
                  setOpen(false)
                }}
                className={cn(
                  'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
                  t === value ? 'text-fg' : 'text-muted',
                )}
              >
                <span className="flex-1 truncate">{t || 'main model'}</span>
                {t === value && <Check size={13} className="text-accent" />}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  )
}

import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { motion, AnimatePresence } from 'framer-motion'
import { cn } from '@/lib/cn'
import { controlSizes, type ControlSize } from './control'

export interface ComboOption {
  value: string
  label: string
  sub?: string
  /** Optional leading mark (e.g. a provider logo). */
  icon?: React.ReactNode
}

// Subsequence fuzzy match: chars of `q` appear in order in `text`.
function fuzzy(q: string, text: string): boolean {
  if (!q) return true
  const s = text.toLowerCase()
  let i = 0
  for (const ch of q.toLowerCase()) {
    i = s.indexOf(ch, i)
    if (i === -1) return false
    i++
  }
  return true
}

// A searchable dropdown for picking option(s). Fuzzy filter as you type.
// Single mode: selecting closes + reports the value. Multi: toggles, stays open.
// allowCreate: the search text becomes a pickable "Create" row (Enter or comma
// commits it) — the tag-input mode.
export function Combobox({
  options,
  selected,
  onChange,
  multiple = false,
  placeholder = 'Select…',
  disabled,
  className,
  size = 'md',
  allowCreate = false,
  triggerLabel,
}: {
  options: ComboOption[]
  selected: string[]
  onChange: (next: string[]) => void
  multiple?: boolean
  placeholder?: string
  disabled?: boolean
  className?: string
  size?: ControlSize
  allowCreate?: boolean
  /** Override the trigger content (e.g. a constant "Add label…" for tag inputs). */
  triggerLabel?: React.ReactNode
}) {
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState('')
  const ref = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  // The panel renders in a body portal (fixed) — ancestors like .mercury-panel
  // create stacking contexts (backdrop-filter), which would cage an absolute
  // dropdown's z-index below the next card. Flips upward near the bottom edge.
  const [pos, setPos] = useState<{ top?: number; bottom?: number; left: number; width: number } | null>(null)

  useEffect(() => {
    if (!open) return
    const place = () => {
      const r = ref.current?.getBoundingClientRect()
      if (!r) return
      const spaceBelow = window.innerHeight - r.bottom
      const flip = spaceBelow < 320 && r.top > spaceBelow
      setPos(
        flip
          ? { bottom: window.innerHeight - r.top + 4, left: r.left, width: r.width }
          : { top: r.bottom + 4, left: r.left, width: r.width },
      )
    }
    place()
    const onClick = (e: MouseEvent) => {
      const t = e.target as Node
      if (!ref.current?.contains(t) && !panelRef.current?.contains(t)) setOpen(false)
    }
    document.addEventListener('mousedown', onClick)
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      document.removeEventListener('mousedown', onClick)
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [open])

  const filtered = useMemo(() => options.filter((o) => fuzzy(q, o.label + ' ' + (o.sub ?? ''))), [options, q])
  const selectedSet = new Set(selected)
  const byValue = (v: string) => options.find((o) => o.value === v)

  const toggle = (v: string) => {
    if (multiple) onChange(selectedSet.has(v) ? selected.filter((x) => x !== v) : [...selected, v])
    else {
      onChange([v])
      setOpen(false)
    }
  }

  // A trimmed query that matches no existing option can be created.
  const creatable = allowCreate ? q.trim().replace(/,+$/, '') : ''
  const canCreate =
    !!creatable && !options.some((o) => o.value.toLowerCase() === creatable.toLowerCase()) && !selectedSet.has(creatable)

  const create = () => {
    if (!canCreate) return
    toggle(creatable)
    setQ('')
  }

  const onSearchKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      // Close just the dropdown — without this, the event reaches the Modal's
      // document-level listener and closes the whole dialog (losing edits).
      e.preventDefault()
      e.stopPropagation()
      setOpen(false)
      return
    }
    if (!allowCreate) return
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      create()
    }
  }

  const defaultTriggerLabel = () => {
    if (selected.length === 0) return <span className="text-muted">{placeholder}</span>
    if (!multiple) {
      const o = byValue(selected[0]!)
      return (
        <span className="flex min-w-0 items-center gap-2">
          {o?.icon}
          <span className="truncate text-fg">{o?.label ?? selected[0]}</span>
        </span>
      )
    }
    const labels = selected.map((v) => byValue(v)?.label ?? v)
    return (
      <span className="truncate text-fg">
        {labels.slice(0, 2).join(', ')}
        {labels.length > 2 ? ` +${labels.length - 2}` : ''}
      </span>
    )
  }

  return (
    <div ref={ref} className={cn('relative', className)}>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        className={cn(
          controlSizes[size],
          'flex w-full items-center gap-2 rounded-xl border border-line bg-[var(--theme-input)] px-2.5 text-sm outline-none transition-colors hover:border-[var(--theme-accent-border)] disabled:opacity-50',
        )}
      >
        <span className="min-w-0 flex-1 text-left">{triggerLabel ?? defaultTriggerLabel()}</span>
        <span className="text-muted">▾</span>
      </button>

      {typeof document !== 'undefined' &&
        createPortal(
          <AnimatePresence>
          {open && pos && (
            <motion.div
              ref={panelRef}
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.14 }}
              className="mercury-panel fixed z-[60] overflow-hidden rounded-xl"
              style={{ top: pos.top, bottom: pos.bottom, left: pos.left, width: pos.width }}
            >
            <input
              autoFocus
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={onSearchKeyDown}
              placeholder={allowCreate ? 'Search or create…' : 'Search…'}
              className="w-full border-b border-line-subtle bg-transparent px-3 py-2 text-sm text-fg outline-none placeholder:text-muted"
            />
            <ul className="max-h-56 overflow-y-auto p-1">
              {filtered.length === 0 && !canCreate && <li className="px-2 py-2 text-xs text-muted">No matches</li>}
              {canCreate && (
                <li>
                  <button
                    type="button"
                    onClick={create}
                    className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm text-fg transition-colors hover:bg-card2"
                  >
                    <span className="text-accent">＋</span> Create “{creatable}”
                  </button>
                </li>
              )}
              {filtered.map((o) => (
                <li key={o.value}>
                  <button
                    type="button"
                    onClick={() => toggle(o.value)}
                    className={cn(
                      'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card2',
                      selectedSet.has(o.value) && 'bg-card2',
                    )}
                  >
                    {o.icon}
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-fg">{o.label}</span>
                      {o.sub && <span className="block truncate text-xs text-muted">{o.sub}</span>}
                    </span>
                    {selectedSet.has(o.value) && <span className="text-accent">✓</span>}
                  </button>
                </li>
              ))}
            </ul>
            </motion.div>
          )}
          </AnimatePresence>,
          document.body,
        )}
    </div>
  )
}

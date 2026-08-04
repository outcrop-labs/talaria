import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { motion, AnimatePresence } from 'framer-motion'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { Skeleton } from '@/components/ui/skeleton'
import { MeterBars, PopSearch, chipPrimary, focusGold, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'
import type { AgentModel } from '@/lib/agents'

// The agent switcher — pick which fleet agent you're talking to.
export function AgentPicker({
  agents,
  value,
  onChange,
  loading,
  fullWidth,
}: {
  agents: AgentModel[]
  value: string | null
  onChange: (id: string) => void
  loading?: boolean
  fullWidth?: boolean
}) {
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState('')
  const ref = useRef<HTMLDivElement>(null)
  const current = agents.find((a) => a.id === value) ?? null

  useEffect(() => {
    if (!open) return
    const onDocClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDocClick)
    return () => document.removeEventListener('mousedown', onDocClick)
  }, [open])

  const needle = q.trim().toLowerCase()
  const visible = agents.filter(
    (a) => !needle || a.label.toLowerCase().includes(needle) || a.role?.toLowerCase().includes(needle),
  )
  return (
    <div ref={ref} className={cn('relative', fullWidth && 'w-full')}>
      {/* Select-shaped trigger (spec §8): raised tile, hairline border, radius 6. */}
      <button
        type="button"
        disabled={loading || agents.length === 0}
        onClick={() => {
          setQ('')
          setOpen((o) => !o)
        }}
        className={cn(
          'flex items-center gap-2 rounded-md border border-line bg-raised font-sans text-sm transition-colors hover:border-line-strong disabled:opacity-60',
          focusGold,
          fullWidth ? 'w-full px-2 py-1.5' : 'py-1 pl-1 pr-3',
        )}
      >
        {loading ? (
          // Picker-shaped shimmer while the fleet loads (button stays disabled).
          <>
            <Skeleton className="h-7 w-7 shrink-0 rounded-full" />
            <span className="min-w-0 flex-1 space-y-1.5 text-left">
              <Skeleton className="h-2.5 w-24 rounded-full" delay={0.12} />
              <Skeleton className="h-2 w-16 rounded-full" delay={0.24} />
            </span>
          </>
        ) : (
          <>
            <Avatar name={current?.label} />
            <span className="min-w-0 flex-1 text-left">
              <span className="block truncate text-fg">{current ? current.label : 'Select an agent'}</span>
              {current?.role && <span className="block truncate text-xs text-muted">{current.role}</span>}
            </span>
          </>
        )}
        <span className="text-muted">▾</span>
      </button>

      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.15 }}
            className={cn(popPanel, 'absolute left-0 z-20 mt-2', fullWidth ? 'w-full' : 'w-64')}
          >
            {/* §7 popover pattern: search field on top (mono placeholder, ⌘K hint). */}
            <PopSearch value={q} onChange={setQ} placeholder="Search agents" />
            <ul className="max-h-72 overflow-auto">
              {visible.map((a) => (
                <li key={a.id}>
                  <button
                    type="button"
                    onClick={() => {
                      onChange(a.id)
                      setOpen(false)
                    }}
                    className={cn(popRow, 'py-2', a.id === value ? popRowSelected : 'text-muted')}
                  >
                    <Avatar name={a.label} />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-fg">{a.label}</span>
                      {a.role && (
                        <span className="block truncate font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
                          {a.role}
                        </span>
                      )}
                    </span>
                    {a.id === value && <span aria-hidden className="h-[7px] w-[7px] shrink-0 rounded-full bg-accent" />}
                  </button>
                </li>
              ))}
              {visible.length === 0 && <li className="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</li>}
            </ul>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

/** The composer rail's agent chip (spec §7): SCOUT-style — strong border,
 *  mono uppercase agent name, 3×12 meter marking where the pick sits in the
 *  fleet (capped at the spec's five bars) — opening the §7 popover (search
 *  row with ⌘K hint, mono header, right-aligned mono role meta, dashed-gold
 *  selected row). A conversation is bound to its agent, so picking here hands
 *  off to the host's route-level agent selection (same behavior as picking
 *  the agent in the sidebar — the host swaps to that agent's thread). */
export function AgentChip({
  agents,
  value,
  onChange,
  className,
}: {
  agents: { id: string; label: string; role?: string }[]
  value: string
  onChange: (id: string) => void
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

  const current = agents.find((a) => a.id === value)
  const total = Math.min(agents.length, 5)
  const meterLit = (id: string) => {
    const i = Math.max(0, agents.findIndex((a) => a.id === id))
    return Math.max(1, Math.round(((i + 1) / Math.max(1, agents.length)) * total))
  }
  const needle = q.trim().toLowerCase()
  const visible = agents.filter(
    (a) => !needle || a.label.toLowerCase().includes(needle) || a.role?.toLowerCase().includes(needle),
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
        className={cn(chipPrimary, className)}
        title="Agent for this conversation"
      >
        <span className="max-w-28 truncate">{current?.label ?? value}</span>
        <MeterBars total={total} lit={meterLit(value)} />
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
            <PopSearch value={q} onChange={setQ} placeholder="Search agents" />
            <div className={popHeader}>Agent</div>
            <div className="max-h-72 overflow-auto">
              {visible.map((a) => (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => {
                    setOpen(false)
                    if (a.id !== value) onChange(a.id)
                  }}
                  className={cn(popRow, a.id === value ? popRowSelected : 'text-muted')}
                >
                  <span className="min-w-0 flex-1 truncate">{a.label}</span>
                  {a.role && (
                    <span className="max-w-40 shrink-0 truncate text-right font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
                      {a.role}
                    </span>
                  )}
                  <MeterBars total={total} lit={meterLit(a.id)} className="shrink-0" />
                </button>
              ))}
              {visible.length === 0 && <div className="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>}
            </div>
          </div>,
          document.body,
        )}
    </>
  )
}

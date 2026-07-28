import { useEffect, useRef, useState } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { Skeleton } from '@/components/ui/skeleton'
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

  return (
    <div ref={ref} className={cn('relative', fullWidth && 'w-full')}>
      <button
        type="button"
        disabled={loading || agents.length === 0}
        onClick={() => setOpen((o) => !o)}
        className={cn(
          'flex items-center gap-2 border border-line bg-card text-sm transition-colors hover:border-[var(--theme-accent-border)] disabled:opacity-60',
          fullWidth ? 'w-full rounded-xl px-2 py-1.5' : 'rounded-full py-1 pl-1 pr-3',
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
          <motion.ul
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.15 }}
            className={cn(
              'mercury-panel absolute left-0 z-20 mt-2 max-h-80 overflow-auto rounded-xl p-1',
              fullWidth ? 'w-full' : 'w-64',
            )}
          >
            {agents.map((a) => (
              <li key={a.id}>
                <button
                  type="button"
                  onClick={() => {
                    onChange(a.id)
                    setOpen(false)
                  }}
                  className={cn(
                    'flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left text-sm transition-colors hover:bg-card2',
                    a.id === value && 'bg-card2',
                  )}
                >
                  <Avatar name={a.label} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-fg">{a.label}</span>
                    {a.role && <span className="block truncate text-xs text-muted">{a.role}</span>}
                  </span>
                  {a.id === value && <span className="text-accent">●</span>}
                </button>
              </li>
            ))}
          </motion.ul>
        )}
      </AnimatePresence>
    </div>
  )
}

import { useState, type ReactNode } from 'react'
import { motion, AnimatePresence } from 'framer-motion'
import { cn } from '@/lib/cn'

// A clean collapsible section — collapsed by default. Reuse for thinking traces,
// tool calls, and any other secondary detail that shouldn't clutter the thread.
// Gentle dew (spec §10): compact mono trigger row on a raised hairline card.
export function Disclosure({
  title,
  icon,
  defaultOpen = false,
  className,
  children,
}: {
  title: ReactNode
  icon?: ReactNode
  defaultOpen?: boolean
  className?: string
  children: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <div className={cn('overflow-hidden rounded-md border border-line bg-card', className)}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2 px-3 py-1.5 text-left font-mono text-[11px] tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg"
      >
        <motion.span animate={{ rotate: open ? 90 : 0 }} transition={{ duration: 0.15 }} className="text-[10px]">
          ▶
        </motion.span>
        {icon}
        <span className="min-w-0 flex-1 truncate">{title}</span>
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.18 }}
          >
            <div className="border-t border-line px-3 py-2">{children}</div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

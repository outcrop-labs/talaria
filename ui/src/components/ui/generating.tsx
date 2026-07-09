import { cn } from '@/lib/cn'

// Generation-in-progress: an agent is writing something and the result will
// replace this block. Shimmering skeleton lines shaped like the coming content
// plus a stepped-dot status line — calmer than a spinner, honest about
// indeterminate work. Use `lines={0}` for just the status line.
export function Generating({ label, lines = 3, className }: { label?: string; lines?: number; className?: string }) {
  const widths = ['85%', '100%', '72%', '94%', '63%', '88%']
  return (
    <div className={cn('rounded-xl border border-line-subtle p-4', className)}>
      {label && (
        <div className="flex items-center gap-2 text-sm text-muted">
          <GeneratingDots />
          <span>{label}</span>
        </div>
      )}
      {lines > 0 && (
        <div className={cn('space-y-2.5', label && 'mt-3.5')}>
          {Array.from({ length: lines }, (_, i) => (
            <div
              key={i}
              className="h-2.5 animate-pulse rounded-full"
              style={{
                width: widths[i % widths.length],
                background: 'var(--theme-line)',
                animationDelay: `${i * 0.18}s`,
              }}
            />
          ))}
        </div>
      )}
    </div>
  )
}

/** The stepped thinking dots on their own (status rows, buttons). */
export function GeneratingDots({ className }: { className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-1', className)}>
      {[0, 0.15, 0.3].map((d) => (
        <span key={d} className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent" style={{ animationDelay: `${d}s` }} />
      ))}
    </span>
  )
}

/** A veil over content being rewritten in place (e.g. the plan document while
 *  the agent resyncs it). Parent must be `relative`. */
export function GeneratingOverlay({ label }: { label: string }) {
  return (
    <div className="absolute inset-0 z-10 grid place-items-center bg-[color:var(--theme-bg)]/60 backdrop-blur-[2px]">
      <Generating label={label} lines={4} className="mercury-panel w-72 border-0" />
    </div>
  )
}

import { cn } from '@/lib/cn'

// Loading skeletons — the shape of the coming content, shimmering. Same idiom
// as Generating (staggered pulse bars on --theme-line) but for FETCHES: use
// these instead of a blank pane or a "Loading" string wherever a query hasn't
// resolved yet. Rule of thumb: the skeleton should roughly match the layout
// it's standing in for, so the swap doesn't jump.

/** One shimmer block. Size it with className (h-*, w-*). */
export function Skeleton({ className, delay = 0 }: { className?: string; delay?: number }) {
  return (
    <div
      aria-hidden
      className={cn('animate-pulse rounded-lg', className)}
      style={{ background: 'var(--theme-line)', animationDelay: `${delay}s` }}
    />
  )
}

/** N list rows: optional leading dot/avatar circle + a text bar. For sidebars,
 *  queues, tables — anything row-shaped. */
export function SkeletonRows({ rows = 4, avatar = false, className }: { rows?: number; avatar?: boolean; className?: string }) {
  const widths = ['70%', '85%', '55%', '78%', '62%', '88%']
  return (
    <div aria-hidden className={cn('space-y-3', className)}>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-2.5">
          {avatar && <Skeleton className="h-6 w-6 shrink-0 rounded-full" delay={i * 0.12} />}
          <div style={{ width: widths[i % widths.length] }}>
            <Skeleton className="h-2.5 w-full rounded-full" delay={i * 0.12} />
          </div>
        </div>
      ))}
    </div>
  )
}

/** A card-shaped placeholder: title bar + a couple of text lines. Drop into
 *  the same grid the real cards will occupy. */
export function SkeletonCard({ className, delay = 0 }: { className?: string; delay?: number }) {
  return (
    <div aria-hidden className={cn('mercury-panel space-y-3 rounded-xl p-4', className)}>
      <Skeleton className="h-3 w-1/2 rounded-full" delay={delay} />
      <Skeleton className="h-2.5 w-full rounded-full" delay={delay + 0.12} />
      <Skeleton className="h-2.5 w-2/3 rounded-full" delay={delay + 0.24} />
    </div>
  )
}

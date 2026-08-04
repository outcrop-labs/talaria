// The one panel/section header — the §8 canonical: 10px mono UPPERCASE
// tracking 0.08em ink-dim, with optional InfoTip and a right-aligned mono
// meta/action slot. Replaces the 19 hand-rolled `flex items-center gap-1.5`
// clusters.
import { cn } from '@/lib/cn'
import { InfoTip } from './info-tip'

export function SectionHeader({
  title,
  info,
  action,
  className,
}: {
  title: React.ReactNode
  /** InfoTip copy — omit for no tip. */
  info?: string
  /** Right-aligned control or mono meta (button, chip, toggle, `08 LIVE`). */
  action?: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('mb-3 flex min-h-6 items-center gap-1.5', className)}>
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{title}</span>
      {info && <InfoTip text={info} />}
      {action && (
        <span className="ml-auto flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
          {action}
        </span>
      )}
    </div>
  )
}

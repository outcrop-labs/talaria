// The one panel/section header: title + optional InfoTip + optional trailing
// action. Replaces the 19 hand-rolled `flex items-center gap-1.5` clusters.
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
  /** Right-aligned control (button, chip, toggle). */
  action?: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('mb-3 flex items-center gap-1.5', className)}>
      <span className="text-sm font-semibold text-fg">{title}</span>
      {info && <InfoTip text={info} />}
      {action && <span className="ml-auto">{action}</span>}
    </div>
  )
}

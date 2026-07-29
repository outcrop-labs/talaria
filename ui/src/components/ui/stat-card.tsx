import { Panel } from '@/components/ui/panel'
import { cn } from '@/lib/cn'

// A labeled metric tile. Reuse for fleet/cost/activity summaries.
export function StatCard({ label, value, sub, className }: { label: string; value: React.ReactNode; sub?: string; className?: string }) {
  return (
    <Panel className={cn('p-5', className)}>
      <div className="text-xs uppercase tracking-wide text-muted">{label}</div>
      <div className="mercury-text mt-1 text-2xl font-semibold">{value}</div>
      {sub && <div className="mt-0.5 text-xs text-muted">{sub}</div>}
    </Panel>
  )
}

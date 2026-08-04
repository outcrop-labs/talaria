import { Panel } from '@/components/ui/panel'
import { cn } from '@/lib/cn'

// The §8 stat block: big sans numeral in readout cream + 10px mono uppercase
// ink-dim label + optional mono sub line. Reuse for fleet/cost/activity
// summaries.
export function StatCard({ label, value, sub, className }: { label: string; value: React.ReactNode; sub?: string; className?: string }) {
  return (
    <Panel className={cn('p-5', className)}>
      <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
      <div className="mt-1.5 font-sans text-2xl font-semibold text-fg">{value}</div>
      {sub && <div className="mt-1 font-mono text-[11px] text-muted">{sub}</div>}
    </Panel>
  )
}

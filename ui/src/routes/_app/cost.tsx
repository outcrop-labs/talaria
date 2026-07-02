import { createFileRoute } from '@tanstack/react-router'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { StatCard } from '@/components/ui/stat-card'
import { relativeTime } from '@/lib/fleet'
import { agentLabel, formatTokens, useCost, type CostTotals } from '@/lib/cost'

export const Route = createFileRoute('/_app/cost')({
  component: CostPage,
})

// The token ledger: every agent generation (1:1 chat + channel replies) lands in
// usage_events — real gateway-reported counts, or char-based estimates (~).
function CostPage() {
  const { data, isLoading } = useCost()
  const t = data?.totals
  const perAgent = data?.perAgent ?? []
  const perDay = data?.perDay ?? []
  // "~" when the window is dominated by estimates rather than reported usage.
  const approx = (t?.estimatedShare ?? 0) > 0.5 ? '~' : ''
  const total = (x?: CostTotals) => (x ? x.prompt + x.completion : 0)

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="mx-auto max-w-5xl space-y-6">
        <h1 className="mercury-text text-2xl font-semibold">Cost</h1>

        {isLoading ? (
          <div className="text-sm text-muted">Loading ledger…</div>
        ) : !data || total(t?.month) === 0 ? (
          <EmptyState
            icon="⌗"
            title="No usage recorded yet"
            hint="Every chat turn and channel reply lands here once agents start talking."
          />
        ) : (
          <>
            <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
              <StatCard
                label="Tokens today"
                value={approx + formatTokens(total(t?.today))}
                sub={`${formatTokens(t?.today.prompt ?? 0)} in · ${formatTokens(t?.today.completion ?? 0)} out`}
              />
              <StatCard
                label="Tokens · 7 days"
                value={approx + formatTokens(total(t?.week))}
                sub={`${formatTokens(t?.week.prompt ?? 0)} in · ${formatTokens(t?.week.completion ?? 0)} out`}
              />
              <StatCard
                label="Tokens · 30 days"
                value={approx + formatTokens(total(t?.month))}
                sub={`${formatTokens(t?.month.prompt ?? 0)} in · ${formatTokens(t?.month.completion ?? 0)} out`}
              />
              <StatCard label="Generations · 30 days" value={t?.month.generations ?? 0} sub="chat turns + channel replies" />
            </div>

            {perDay.length > 1 && (
              <Panel className="p-4">
                <h2 className="mb-3 text-sm font-semibold text-fg">Tokens per day · last 14 days</h2>
                <DailyBars days={perDay} />
              </Panel>
            )}

            <Panel className="p-4">
              <h2 className="mb-3 text-sm font-semibold text-fg">By agent · 30 days</h2>
              <ul className="divide-y divide-line-subtle">
                {perAgent.map((a) => {
                  const d = agentLabel(a.agentModel)
                  return (
                    <li key={a.agentModel} className="flex items-center gap-3 py-2">
                      <Avatar name={d.label} className="h-6 w-6" />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm text-fg">{d.label}</span>
                        <span className="block truncate text-xs text-muted">{d.role}</span>
                      </span>
                      <span className="w-20 text-right text-xs text-muted">{formatTokens(a.prompt)} in</span>
                      <span className="w-20 text-right text-xs text-muted">{formatTokens(a.completion)} out</span>
                      <span className="w-20 text-right text-sm text-fg">{formatTokens(a.prompt + a.completion)}</span>
                      <span className="w-16 text-right text-xs text-muted">{a.generations} gen</span>
                      <span className="w-20 text-right text-xs text-muted">{relativeTime(a.lastUsed)}</span>
                    </li>
                  )
                })}
              </ul>
            </Panel>

            {(t?.estimatedShare ?? 0) > 0 && (
              <p className="text-xs text-muted">
                ~{Math.round((t?.estimatedShare ?? 0) * 100)}% of generations are character-based estimates — the
                gateway didn't report token usage for them.
              </p>
            )}
          </>
        )}
      </div>
    </div>
  )
}

/** Single-series daily bar strip. One hue (theme accent); exact numbers on hover. */
function DailyBars({ days }: { days: Array<CostTotals & { day: string }> }) {
  const max = Math.max(...days.map((d) => d.prompt + d.completion), 1)
  return (
    <div className="flex items-end gap-1" style={{ height: '5.5rem' }} role="img" aria-label="Tokens per day">
      {days.map((d) => {
        const v = d.prompt + d.completion
        const h = Math.max((v / max) * 100, v > 0 ? 3 : 1)
        return (
          <div key={d.day} className="group flex h-full min-w-0 flex-1 flex-col justify-end text-center">
            <div
              className="mx-auto w-full max-w-7 rounded-t bg-accent transition-all group-hover:brightness-110"
              style={{ height: `${h}%`, opacity: v > 0 ? 1 : 0.25 }}
              title={`${d.day}: ${formatTokens(v)} tokens (${formatTokens(d.prompt)} in, ${formatTokens(d.completion)} out, ${d.generations} generations)`}
            />
            <div className="mt-1 truncate text-[9px] text-muted">
              {new Date(`${d.day}T00:00:00`).toLocaleDateString([], { weekday: 'narrow' })}
            </div>
          </div>
        )
      })}
    </div>
  )
}

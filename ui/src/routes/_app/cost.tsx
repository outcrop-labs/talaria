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

            {t && t.split.local + t.split.cloud > 0 && (
              <Panel className="p-4">
                <h2 className="mb-1 text-sm font-semibold text-fg">Local vs cloud · 30 days</h2>
                <p className="mb-3 text-xs text-muted">
                  {formatTokens(t.split.local)} on your own hardware · {formatTokens(t.split.cloud)} on cloud APIs —{' '}
                  {Math.round((t.split.local / (t.split.local + t.split.cloud)) * 100)}% local
                </p>
                <div className="flex h-3 overflow-hidden rounded-full" role="img" aria-label="Local vs cloud token share">
                  <div
                    style={{
                      width: `${(t.split.local / (t.split.local + t.split.cloud)) * 100}%`,
                      background: 'var(--theme-success)',
                    }}
                  />
                  <div className="ml-0.5 flex-1" style={{ background: 'var(--theme-accent)' }} />
                </div>
                <div className="mt-2 flex gap-4 text-xs text-muted">
                  <span className="inline-flex items-center gap-1.5">
                    <span className="h-2 w-2 rounded-full" style={{ background: 'var(--theme-success)' }} /> local
                  </span>
                  <span className="inline-flex items-center gap-1.5">
                    <span className="h-2 w-2 rounded-full" style={{ background: 'var(--theme-accent)' }} /> cloud
                  </span>
                </div>
              </Panel>
            )}

            {perDay.length > 1 && (
              <Panel className="p-4">
                <div className="mb-3 flex items-baseline gap-4">
                  <h2 className="text-sm font-semibold text-fg">Tokens per day · last 14 days</h2>
                  <span className="inline-flex items-center gap-1.5 text-xs text-muted">
                    <span className="h-2 w-2 rounded-full" style={{ background: 'var(--theme-success)' }} /> local
                  </span>
                  <span className="inline-flex items-center gap-1.5 text-xs text-muted">
                    <span className="h-2 w-2 rounded-full" style={{ background: 'var(--theme-accent)' }} /> cloud
                  </span>
                </div>
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
                      <span className="w-16 text-right text-xs" style={{ color: 'var(--theme-success)' }}>
                        {a.localShare === null ? '—' : `${Math.round(a.localShare * 100)}% local`}
                      </span>
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

/** Daily strip, stacked local (bottom) + cloud (top). Unattributed tokens fall
 *  into the cloud segment's hue at reduced opacity via the remainder. */
function DailyBars({ days }: { days: Array<CostTotals & { day: string; local: number; cloud: number }> }) {
  const max = Math.max(...days.map((d) => d.prompt + d.completion), 1)
  return (
    <div className="flex items-end gap-1" style={{ height: '5.5rem' }} role="img" aria-label="Tokens per day, local vs cloud">
      {days.map((d) => {
        const v = d.prompt + d.completion
        const other = Math.max(v - d.local - d.cloud, 0)
        const title = `${d.day}: ${formatTokens(v)} tokens — ${formatTokens(d.local)} local, ${formatTokens(d.cloud)} cloud${
          other ? `, ${formatTokens(other)} unattributed` : ''
        } (${d.generations} generations)`
        const px = (n: number) => `${Math.max((n / max) * 100, n > 0 ? 2 : 0)}%`
        return (
          <div key={d.day} className="group flex h-full min-w-0 flex-1 flex-col justify-end text-center" title={title}>
            {other > 0 && (
              <div className="mx-auto w-full max-w-7 rounded-t opacity-40" style={{ height: px(other), background: 'var(--theme-accent)' }} />
            )}
            {d.cloud > 0 && (
              <div
                className={`mx-auto w-full max-w-7 ${other ? 'mt-px' : 'rounded-t'}`}
                style={{ height: px(d.cloud), background: 'var(--theme-accent)' }}
              />
            )}
            {d.local > 0 && (
              <div
                className={`mx-auto w-full max-w-7 ${d.cloud || other ? 'mt-px' : 'rounded-t'}`}
                style={{ height: px(d.local), background: 'var(--theme-success)' }}
              />
            )}
            {v === 0 && <div className="mx-auto w-full max-w-7 rounded-t bg-accent opacity-25" style={{ height: '1%' }} />}
            <div className="mt-1 truncate text-[9px] text-muted">
              {new Date(`${d.day}T00:00:00`).toLocaleDateString([], { weekday: 'narrow' })}
            </div>
          </div>
        )
      })}
    </div>
  )
}

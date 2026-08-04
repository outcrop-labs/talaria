import { SkeletonCard, SkeletonRows } from '@/components/ui/skeleton'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { SectionHeader } from '@/components/ui/section-header'
import { StatCard } from '@/components/ui/stat-card'
import { QueryError } from '@/components/ui/query-state'
import { relativeTime } from '@/lib/fleet'
import { agentLabel, formatCost, formatTokens, useCost, type CostOverview, type CostTotals } from '@/lib/cost'


// The token ledger: every agent generation (1:1 chat + channel replies) lands in
// usage_events — real gateway-reported counts, or char-based estimates (~).
export function CostPanel() {
  const costQuery = useCost()
  const { data, isLoading } = costQuery
  const t = data?.totals
  const perAgent = data?.perAgent ?? []
  const perDay = data?.perDay ?? []
  // "~" when the window is dominated by estimates rather than reported usage.
  const approx = (t?.estimatedShare ?? 0) > 0.5 ? '~' : ''
  const total = (x?: CostTotals) => (x ? x.prompt + x.completion : 0)

  // `data` is undefined on rejection, and the old guard read
  // `!data || total(t?.month) === 0` — so a 500 on /api/cost rendered
  // "No usage recorded yet" over a ledger holding hundreds of usage_events.
  // That is a claim about SPEND made from a read that failed. Split the three
  // answers apart: broke, still loading, genuinely zero.
  const hardFailure = costQuery.isError && data === undefined
  // A failed BACKGROUND refetch (this query polls every 60s) keeps the last
  // good ledger on screen — blanking real numbers over a blip is the worse
  // lie — but it must not pass stale totals off as current.
  const staleFailure = costQuery.isError && data !== undefined

  return (
    <div>
      <div className="space-y-8">

        {staleFailure && (
          <QueryError
            variant="inline"
            title="Usage may be out of date"
            error={costQuery.error}
            onRetry={() => void costQuery.refetch()}
          />
        )}

        {hardFailure ? (
          <QueryError
            title="Could not load usage"
            error={costQuery.error}
            onRetry={() => void costQuery.refetch()}
          />
        ) : isLoading || !data ? (
          <>
            <div className="grid grid-cols-3 gap-4">
              {[0, 1, 2].map((i) => (
                <SkeletonCard key={i} delay={i * 0.1} />
              ))}
            </div>
            <SkeletonRows rows={5} className="mt-2" />
          </>
        ) : total(t?.month) === 0 ? (
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
              <StatCard
                label="Cloud spend · 30 days"
                value={formatCost(t?.month.cost ?? 0)}
                sub={`today ${formatCost(t?.today.cost ?? 0)} · ${t?.month.generations ?? 0} generations`}
              />
            </div>

            {/* Missing pricing is attention (gold), not failure — orange stays
                reserved for real failures (spec §1 semantics). */}
            {(t?.unpricedCloudTokens ?? 0) > 0 && (
              <p className="text-xs text-warning">
                {formatTokens(t!.unpricedCloudTokens)} cloud tokens have no price configured. Set per-model pricing on
                the Models page so spend is complete.
              </p>
            )}

            {t && t.split.local + t.split.cloud + t.split.other > 0 && (
              <SplitPanel split={t.split} perModel={data.perModel} />
            )}

            {perDay.length > 1 && (
              <Panel>
                <SectionHeader
                  title="Tokens per day · last 14 days"
                  action={
                    <>
                      <span className="inline-flex items-center gap-1.5">
                        <span className="h-2 w-2 rounded-full" style={{ background: 'var(--theme-success)' }} /> self-hosted
                      </span>
                      <span className="inline-flex items-center gap-1.5">
                        <span className="h-2 w-2 rounded-full" style={{ background: 'var(--theme-chart-1)' }} /> cloud
                      </span>
                    </>
                  }
                />
                <DailyBars days={perDay} />
              </Panel>
            )}

            <Panel>
              <SectionHeader title="By agent · 30 days" action={String(perAgent.length).padStart(2, '0')} />
              <ul className="divide-y divide-line">
                {perAgent.map((a) => {
                  const d = agentLabel(a.agentModel)
                  return (
                    <li key={a.agentModel} className="flex items-center gap-3 py-3 transition-colors hover:bg-hover">
                      <Avatar name={d.label} className="h-6 w-6" />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-sans text-sm text-fg">{d.label}</span>
                        <span className="block truncate font-sans text-xs text-muted">{d.role}</span>
                      </span>
                      <span className="w-20 text-right font-mono text-[11px] text-muted">{formatTokens(a.prompt)} in</span>
                      <span className="w-20 text-right font-mono text-[11px] text-muted">{formatTokens(a.completion)} out</span>
                      <span className="w-16 text-right font-mono text-[11px] text-success">
                        {a.localShare === null ? '—' : `${Math.round(a.localShare * 100)}% self-hosted`}
                      </span>
                      <span className="w-20 text-right font-mono text-sm text-fg">{formatTokens(a.prompt + a.completion)}</span>
                      <span className="w-16 text-right font-mono text-[11px] text-muted">{formatCost(a.cost)}</span>
                      <span className="w-16 text-right font-mono text-[11px] text-muted">{a.generations} gen</span>
                      <span className="w-20 text-right font-mono text-[11px] text-muted">{relativeTime(a.lastUsed)}</span>
                    </li>
                  )
                })}
              </ul>
            </Panel>

            {(t?.estimatedShare ?? 0) > 0 && (
              <p className="text-xs text-muted">
                ~{Math.round((t?.estimatedShare ?? 0) * 100)}% of generations are character-based estimates. The
                gateway didn't report token usage for them.
              </p>
            )}
          </>
        )}
      </div>
    </div>
  )
}

/** Segment colors: hue family = class (green local, chart-blue cloud — gold
 *  stays reserved for action/attention, spec §1), lightness step = model
 *  within the family. Identity is never color-alone — every segment has a
 *  legend chip with the model name and exact tokens. */
const segmentColor = (cls: 'local' | 'cloud' | null, idxInClass: number): string => {
  if (cls === null) return 'var(--theme-line)'
  const base = cls === 'local' ? 'var(--theme-success)' : 'var(--theme-chart-1)'
  const lighten = Math.min(idxInClass * 22, 66)
  return lighten ? `color-mix(in oklab, ${base}, white ${lighten}%)` : base
}

/** The local-vs-cloud share, segmented by serving model. The three class
 *  totals (local + cloud + unattributed) always reconcile with the 30-day
 *  token tile. */
function SplitPanel({
  split,
  perModel,
}: {
  split: { local: number; cloud: number; other: number }
  perModel: CostOverview['perModel']
}) {
  const total = split.local + split.cloud + split.other
  const attributed = split.local + split.cloud
  // Per-model segments in class order; lightness index counted within class.
  let li = 0
  let ci = 0
  const segments = perModel
    .filter((m) => m.tokens > 0)
    .map((m) => ({
      ...m,
      color: segmentColor(m.endpointClass, m.endpointClass === 'local' ? li++ : m.endpointClass === 'cloud' ? ci++ : 0),
      label: m.llmModel ?? 'unattributed',
    }))

  return (
    <Panel>
      <SectionHeader className="mb-2" title="Self-hosted vs cloud · 30 days" />
      <p className="mb-4 font-sans text-xs text-muted">
        {formatTokens(split.local)} on your own hardware · {formatTokens(split.cloud)} on cloud APIs
        {split.other > 0 && ` · ${formatTokens(split.other)} unattributed`}
        {attributed > 0 && ` — ${Math.round((split.local / total) * 100)}% self-hosted`}
      </p>
      <div className="flex h-3 gap-0.5 overflow-hidden rounded-full" role="img" aria-label="Token share by serving model">
        {segments.map((s) => (
          <div
            key={s.label}
            title={`${s.label}: ${formatTokens(s.tokens)} tokens (${s.endpointClass ?? 'unattributed'})`}
            style={{ width: `${(s.tokens / total) * 100}%`, background: s.color, minWidth: s.tokens > 0 ? 4 : 0 }}
          />
        ))}
      </div>
      <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1.5 font-mono text-[11px] text-muted">
        {segments.map((s) => (
          <span key={s.label} className="inline-flex items-center gap-1.5">
            <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: s.color }} />
            <span className="text-fg">{s.label}</span>
            <span>
              {formatTokens(s.tokens)} · {s.endpointClass ?? 'unattributed'}
              {s.endpointClass === 'cloud' && (s.cost === null ? ' · unpriced' : ` · ${formatCost(s.cost)}`)}
            </span>
          </span>
        ))}
      </div>
    </Panel>
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
              <div className="mx-auto w-full max-w-7 rounded-t opacity-40" style={{ height: px(other), background: 'var(--theme-chart-1)' }} />
            )}
            {d.cloud > 0 && (
              <div
                className={`mx-auto w-full max-w-7 ${other ? 'mt-px' : 'rounded-t'}`}
                style={{ height: px(d.cloud), background: 'var(--theme-chart-1)' }}
              />
            )}
            {d.local > 0 && (
              <div
                className={`mx-auto w-full max-w-7 ${d.cloud || other ? 'mt-px' : 'rounded-t'}`}
                style={{ height: px(d.local), background: 'var(--theme-success)' }}
              />
            )}
            {v === 0 && <div className="mx-auto w-full max-w-7 rounded-t bg-card2" style={{ height: '1%' }} />}
            <div className="mt-1 truncate font-mono text-[9px] text-muted">
              {new Date(`${d.day}T00:00:00`).toLocaleDateString([], { weekday: 'narrow' })}
            </div>
          </div>
        )
      })}
    </div>
  )
}

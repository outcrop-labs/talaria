import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import { Panel } from '@/components/ui/panel'
import { Tabs } from '@/components/ui/tabs'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { formatTokens } from '@/lib/cost'
import { ActivityRow } from '@/components/app/activity-row'
import { ComputePanel } from '@/components/observability/inference'
import { CostPanel } from '@/components/observability/cost'
import { AuditPanel } from '@/components/observability/activity'
import { AlertsPanel } from '@/components/observability/alerts'

// Observability — the ops plane in one place. The root is an OVERVIEW (a
// cross-section of alerts, live compute, spend, and the audit pulse); the
// tabs are the detail views the old standalone pages became.
const OBS_TABS = [
  { id: 'overview', label: 'Overview' },
  { id: 'compute', label: 'Compute' },
  { id: 'cost', label: 'Cost' },
  { id: 'audit', label: 'Audit' },
  { id: 'alerts', label: 'Alerts' },
] as const
type ObsTab = (typeof OBS_TABS)[number]['id']

export const Route = createFileRoute('/_app/observability')({
  component: ObservabilityPage,
  // /observability?tab=cost deep-links a detail view; the root is Overview.
  validateSearch: (search: Record<string, unknown>): { tab?: ObsTab } => {
    const t = OBS_TABS.find((v) => v.id === search.tab)
    return t && t.id !== 'overview' ? { tab: t.id } : {}
  },
})

function ObservabilityPage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const tab: ObsTab = search.tab ?? 'overview'
  const setTab = (t: ObsTab) => void navigate({ search: t === 'overview' ? {} : { tab: t } })

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <h1 className="mercury-text text-2xl font-semibold">Observability</h1>
        <Tabs items={OBS_TABS} value={tab} onChange={setTab} />
        {tab === 'overview' && <OverviewPanel onOpen={setTab} />}
        {tab === 'compute' && <ComputePanel />}
        {tab === 'cost' && <CostPanel />}
        {tab === 'audit' && <AuditPanel />}
        {tab === 'alerts' && <AlertsPanel />}
      </div>
    </div>
  )
}

// ── Overview: the cross-section — worst news first ──────────────────────────
interface OverviewAlert {
  severity: 'critical' | 'warning' | 'info'
  title: string
  href: string
}
const SEV_COLOR: Record<OverviewAlert['severity'], string> = {
  critical: 'var(--theme-danger)',
  warning: 'var(--theme-warning)',
  info: 'var(--theme-line)',
}

function OverviewPanel({ onOpen }: { onOpen: (t: ObsTab) => void }) {
  const nav = useNavigate()
  const { data: alerts, isLoading: alertsLoading } = useQuery({
    queryKey: ['alerts'],
    queryFn: async (): Promise<OverviewAlert[]> => {
      const r = await fetch('/api/alerts', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { alerts: OverviewAlert[] }).alerts
    },
    refetchInterval: 60_000,
  })
  const { data: inference } = useQuery({
    queryKey: ['inference'],
    queryFn: async () => {
      const r = await fetch('/api/inference', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json() as Promise<{
        live: {
          generating: Array<{ agentModel: string; count: number }>
          gateway: { requests: number; errors: number; p50: number | null }
          fleet: { running: number; warming: number; unhealthy: number; down: number }
        }
      }>
    },
    refetchInterval: 15_000,
  })
  const { data: cost } = useQuery({
    queryKey: ['cost'],
    queryFn: async () => {
      const r = await fetch('/api/cost', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json() as Promise<{ totals: { today: { prompt: number; completion: number; cost: number }; month: { cost: number } } }>
    },
    refetchInterval: 60_000,
  })
  const { data: audit, isLoading: auditLoading } = useQuery({
    queryKey: ['home-activity', ''],
    queryFn: async () => {
      const r = await fetch('/api/activity', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { events: Array<{ at: string; actor: string; detail: string; context: string; href: string }> }).events
    },
    refetchInterval: 30_000,
  })

  const live = inference?.live
  const generating = live?.generating.reduce((n, g) => n + g.count, 0) ?? 0
  const fleetParts = live
    ? [
        `${live.fleet.running} up`,
        ...(live.fleet.warming ? [`${live.fleet.warming} warming`] : []),
        ...(live.fleet.unhealthy ? [`${live.fleet.unhealthy} unhealthy`] : []),
        ...(live.fleet.down ? [`${live.fleet.down} down`] : []),
      ].join(' · ')
    : null
  const problems = (alerts ?? []).filter((a) => a.severity !== 'info')

  return (
    <div className="space-y-6">
      {/* Worst news first: alerts strip */}
      <Panel>
        <button type="button" onClick={() => onOpen('alerts')} className="mb-3 flex w-full items-center gap-2 text-left">
          <span className="text-sm font-semibold text-fg">Alerts</span>
          {alertsLoading ? (
            <Skeleton className="h-3 w-8 rounded-full" />
          ) : problems.length > 0 ? (
            <span className="rounded-full bg-[color:var(--theme-warning)]/15 px-1.5 text-[10px] font-semibold text-[color:var(--theme-warning)]">{problems.length}</span>
          ) : (
            <span className="text-xs" style={{ color: 'var(--theme-success)' }}>all clear</span>
          )}
          <span className="ml-auto text-xs text-accent">Open →</span>
        </button>
        {alertsLoading ? (
          <SkeletonRows rows={2} />
        ) : problems.length > 0 ? (
          <ul className="space-y-1.5">
            {problems.slice(0, 3).map((a, i) => (
              <li key={i} className="flex items-center gap-2.5 text-sm">
                <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: SEV_COLOR[a.severity] }} />
                <span className="min-w-0 flex-1 truncate font-sans text-fg">{a.title}</span>
              </li>
            ))}
          </ul>
        ) : null}
      </Panel>

      <div className="grid gap-4 sm:grid-cols-3">
        <Panel className="p-4">
          <button type="button" onClick={() => onOpen('compute')} className="w-full text-left">
            <div className="text-xs uppercase tracking-wide text-muted">Generating now</div>
            {live ? (
              <>
                <div className="mt-1 text-xl font-semibold text-fg">{generating}</div>
                <div className="mt-0.5 truncate text-[11px] text-muted">{fleetParts}</div>
              </>
            ) : (
              <Skeleton className="mt-2 h-5 w-16 rounded-full" />
            )}
          </button>
        </Panel>
        <Panel className="p-4">
          <button type="button" onClick={() => onOpen('compute')} className="w-full text-left">
            <div className="text-xs uppercase tracking-wide text-muted">Gateway · 15 min</div>
            {live ? (
              <>
                <div className="mt-1 text-xl font-semibold text-fg">{live.gateway.requests}</div>
                <div className="mt-0.5 text-[11px] text-muted">
                  {live.gateway.errors ? `${live.gateway.errors} errors` : 'no errors'}
                  {live.gateway.p50 != null && ` · p50 ${(live.gateway.p50 / 1000).toFixed(1)}s`}
                </div>
              </>
            ) : (
              <Skeleton className="mt-2 h-5 w-16 rounded-full" />
            )}
          </button>
        </Panel>
        <Panel className="p-4">
          <button type="button" onClick={() => onOpen('cost')} className="w-full text-left">
            <div className="text-xs uppercase tracking-wide text-muted">Spend today</div>
            {cost ? (
              <>
                <div className="mt-1 text-xl font-semibold text-fg">${cost.totals.today.cost.toFixed(2)}</div>
                <div className="mt-0.5 text-[11px] text-muted">
                  {formatTokens(cost.totals.today.prompt + cost.totals.today.completion)} tokens · ${cost.totals.month.cost.toFixed(2)} this month
                </div>
              </>
            ) : (
              <Skeleton className="mt-2 h-5 w-16 rounded-full" />
            )}
          </button>
        </Panel>
      </div>

      <Panel>
        <button type="button" onClick={() => onOpen('audit')} className="mb-3 flex w-full items-center gap-2 text-left">
          <span className="text-sm font-semibold text-fg">Audit trail</span>
          <span className="ml-auto text-xs text-accent">Open →</span>
        </button>
        {auditLoading ? (
          <SkeletonRows rows={5} />
        ) : (audit ?? []).length === 0 ? (
          <div className="text-xs text-muted">Quiet so far.</div>
        ) : (
          <ul className="space-y-1">
            {audit!.slice(0, 6).map((a, i) => (
              <ActivityRow key={i} actor={a.actor} detail={a.detail} at={a.at} onClick={() => a.href && void nav({ to: a.href })} />
            ))}
          </ul>
        )}
      </Panel>
    </div>
  )
}

import { useNavigate } from '@tanstack/react-router'
import { SkeletonRows } from '@/components/ui/skeleton'
import { useQuery } from '@tanstack/react-query'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { QueryState } from '@/components/ui/query-state'
import { getList } from '@/lib/fetch-json'


type Severity = 'critical' | 'warning' | 'info'

interface Alert {
  severity: Severity
  title: string
  detail: string
  href: string
}

const SEV: Record<Severity, { label: string; color: string; icon: string }> = {
  critical: { label: 'Critical', color: 'var(--theme-danger)', icon: '▲' },
  warning: { label: 'Warning', color: 'var(--theme-warning)', icon: '◆' },
  info: { label: 'Info', color: 'var(--theme-accent)', icon: '●' },
}

function useAlerts() {
  return useQuery({
    queryKey: ['alerts'],
    queryFn: (): Promise<Alert[]> => getList<Alert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  })
}

// Derived health signals — computed live from container state, the gateway,
// the token ledger, and stuck tickets. Nothing to configure, nothing stored.
export function AlertsPanel() {
  const query = useAlerts()
  const navigate = useNavigate()

  return (
    <div>
      <div className="space-y-8">
        {/* "All clear" is the single most dangerous thing this app can say
            wrongly — it is the sentence an operator uses to decide NOT to look.
            It may only render off a 200. */}
        <QueryState
          query={query}
          errorTitle="Could not check for alerts"
          skeleton={<SkeletonRows rows={4} avatar />}
          empty={
            <EmptyState
              icon="△"
              title="All clear"
              hint="Agents running, gateway answering, ledger priced, no stuck work."
            />
          }
        >
          {(alerts) => (
          <Panel className="p-0">
            <div className="divide-y divide-line">
              {alerts.map((a, i) => (
                <button
                  key={i}
                  type="button"
                  onClick={() => void navigate({ to: a.href })}
                  className="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors hover:bg-hover"
                >
                  <span className="mt-0.5 w-5 shrink-0 text-center font-mono text-xs" style={{ color: SEV[a.severity].color }}>
                    {SEV[a.severity].icon}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline gap-2">
                      <span className="truncate font-sans text-sm font-medium text-fg">{a.title}</span>
                      <span
                        className="shrink-0 rounded border border-line px-1 font-mono text-[10px] uppercase tracking-[0.05em]"
                        style={{ color: SEV[a.severity].color }}
                      >
                        {SEV[a.severity].label}
                      </span>
                    </div>
                    <div className="font-sans text-sm text-muted">{a.detail}</div>
                  </div>
                </button>
              ))}
            </div>
          </Panel>
          )}
        </QueryState>
      </div>
    </div>
  )
}

import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'

export const Route = createFileRoute('/_app/alerts')({
  component: AlertsPage,
})

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
    queryFn: async (): Promise<Alert[]> => {
      const r = await fetch('/api/alerts')
      if (!r.ok) throw new Error('failed to load alerts')
      return ((await r.json()) as { alerts: Alert[] }).alerts
    },
    refetchInterval: 60_000,
  })
}

// Derived health signals — computed live from container state, the gateway,
// the token ledger, and stuck tickets. Nothing to configure, nothing stored.
function AlertsPage() {
  const { data: alerts = [], isLoading } = useAlerts()
  const navigate = useNavigate()

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-8">
        <h1 className="mercury-text text-2xl font-semibold">Alerts</h1>

        {isLoading ? (
          <div className="text-sm text-muted">Checking the fleet</div>
        ) : alerts.length === 0 ? (
          <EmptyState
            icon="△"
            title="All clear"
            hint="Agents running, gateway answering, ledger priced, no stuck work."
          />
        ) : (
          <Panel className="p-0">
            <div className="divide-y divide-line-subtle">
              {alerts.map((a, i) => (
                <button
                  key={i}
                  type="button"
                  onClick={() => void navigate({ to: a.href })}
                  className="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors hover:bg-card"
                >
                  <span className="mt-0.5 w-5 shrink-0 text-center text-xs" style={{ color: SEV[a.severity].color }}>
                    {SEV[a.severity].icon}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline gap-2">
                      <span className="truncate text-sm font-medium text-fg">{a.title}</span>
                      <span
                        className="shrink-0 rounded border px-1 text-[10px] uppercase tracking-wide"
                        style={{ color: SEV[a.severity].color, borderColor: 'var(--theme-line-subtle)' }}
                      >
                        {SEV[a.severity].label}
                      </span>
                    </div>
                    <div className="text-sm text-muted">{a.detail}</div>
                  </div>
                </button>
              ))}
            </div>
          </Panel>
        )}
      </div>
    </div>
  )
}

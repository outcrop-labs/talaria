import { useNavigate } from '@tanstack/react-router'
import { SkeletonRows } from '@/components/ui/skeleton'
import { useQuery } from '@tanstack/react-query'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'


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
export function AlertsPanel() {
  const { data: alerts = [], isLoading } = useAlerts()
  const navigate = useNavigate()

  return (
    <div>
      <div className="space-y-8">

        {isLoading ? (
          <SkeletonRows rows={4} avatar />
        ) : alerts.length === 0 ? (
          <EmptyState
            icon="△"
            title="All clear"
            hint="Agents running, gateway answering, ledger priced, no stuck work."
          />
        ) : (
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
      </div>
    </div>
  )
}

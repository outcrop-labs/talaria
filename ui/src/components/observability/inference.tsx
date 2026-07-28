import { Link } from '@tanstack/react-router'
import { SkeletonCard } from '@/components/ui/skeleton'
import { useQuery } from '@tanstack/react-query'
import { EmptyState } from '@/components/ui/empty-state'
import { GeneratingDots } from '@/components/ui/generating'
import { Panel } from '@/components/ui/panel'
import { StatCard } from '@/components/ui/stat-card'
import { formatTokens } from '@/lib/cost'
import { relativeTime } from '@/lib/fleet'
import { useSession } from '@/lib/session'


interface LocalBackend {
  id: string
  name: string
  baseUrl: string | null
  models: string[]
  health: { ok: boolean; latencyMs: number | null; servingNow: string[]; note: string | null }
}

interface InferenceLive {
  generating: Array<{ agentModel: string; count: number }>
  lastHour: Array<{ agentModel: string; generations: number; tokens: number; lastAt: string }>
  gateway: { requests: number; errors: number; p50: number | null; p95: number | null }
  fleet: { running: number; warming: number; unhealthy: number; down: number }
}

interface InferenceData {
  live: InferenceLive
  backends: LocalBackend[]
  usage: { today: number; month: number; generations: number; perModel: Array<{ llmModel: string | null; tokens: number }> }
}

const useInference = () =>
  useQuery({
    queryKey: ['inference'],
    queryFn: async (): Promise<InferenceData> => {
      const r = await fetch('/api/inference')
      if (!r.ok) throw new Error('failed to load')
      return r.json()
    },
    // Lean in while something is actually generating or warming.
    refetchInterval: (q) => {
      const live = q.state.data?.live
      return live && (live.generating.length > 0 || live.fleet.warming > 0) ? 5_000 : 15_000
    },
  })

/** The live strip: generating-now, gateway pulse, fleet temperature, and the
 *  last hour of per-agent activity. */
function LiveSection({ live }: { live: InferenceLive }) {
  const generatingTotal = live.generating.reduce((n, g) => n + g.count, 0)
  const genByAgent = new Map(live.generating.map((g) => [g.agentModel, g.count]))
  const fleetParts = [
    `${live.fleet.running} up`,
    ...(live.fleet.warming ? [`${live.fleet.warming} warming`] : []),
    ...(live.fleet.unhealthy ? [`${live.fleet.unhealthy} unhealthy`] : []),
    ...(live.fleet.down ? [`${live.fleet.down} down`] : []),
  ]
  return (
    <>
      <div className="grid grid-cols-4 gap-4">
        <StatCard
          label="Generating now"
          value={
            generatingTotal ? (
              <span className="flex items-center gap-2">
                {generatingTotal} <GeneratingDots />
              </span>
            ) : (
              '0'
            )
          }
          sub={generatingTotal ? `${live.generating.length} agent${live.generating.length === 1 ? '' : 's'}` : 'all quiet'}
        />
        <StatCard
          label="Gateway · 15 min"
          value={String(live.gateway.requests)}
          sub={live.gateway.errors ? `${live.gateway.errors} errors` : 'no errors'}
        />
        <StatCard
          label="First byte"
          value={live.gateway.p50 != null ? `${(live.gateway.p50 / 1000).toFixed(1)}s` : '—'}
          sub={live.gateway.p95 != null ? `p95 ${(live.gateway.p95 / 1000).toFixed(1)}s` : 'no recent requests'}
        />
        <StatCard label="Fleet" value={fleetParts[0]!} sub={fleetParts.slice(1).join(' · ') || 'all healthy'} />
      </div>
      {live.lastHour.length > 0 && (
        <Panel>
          <div className="mb-4 text-sm font-semibold text-fg">Active this hour</div>
          <div className="divide-y divide-line-subtle">
            {live.lastHour.map((a) => (
              <div key={a.agentModel} className="flex items-center gap-3 py-3 text-sm">
                <span className="min-w-0 flex-1 truncate text-fg">{a.agentModel}</span>
                {genByAgent.has(a.agentModel) && <GeneratingDots />}
                <span className="shrink-0 text-muted">{a.generations} gen</span>
                <span className="shrink-0 text-muted">{formatTokens(a.tokens)} tokens</span>
                <span className="w-16 shrink-0 text-right text-xs text-muted">{relativeTime(a.lastAt)}</span>
              </div>
            ))}
          </div>
        </Panel>
      )}
    </>
  )
}

// The inference plane, live: what's generating right now, the gateway's pulse,
// fleet container temperature — then the self-hosted backends underneath.
export function ComputePanel() {
  const { data: session } = useSession()
  const { data, isLoading } = useInference()
  const backends = data?.backends ?? []
  const live = data?.live

  return (
    <div>
      <div className="space-y-8">
        <div className="flex items-center gap-3">
          {session?.role === 'admin' && (
            <Link to="/models" className="ml-auto text-sm text-accent hover:underline">
              Configure on Models →
            </Link>
          )}
        </div>

        {live ? (
          <LiveSection live={live} />
        ) : (
          <div className="grid grid-cols-4 gap-4">
            {[0, 1, 2, 3].map((i) => (
              <SkeletonCard key={i} delay={i * 0.1} />
            ))}
          </div>
        )}

        <div className="mercury-text text-lg font-semibold">Self-hosted compute</div>

        {isLoading ? (
          <SkeletonCard />
        ) : backends.length === 0 ? (
          <EmptyState
            icon="▦"
            title="No self-hosted backends"
            hint="Add a self-hosted provider (Ollama, vLLM, a router) on the Models page. LAN and loopback URLs are classed self-hosted automatically."
          />
        ) : (
          <>
            <div className="grid grid-cols-3 gap-4">
              <StatCard label="Self-hosted tokens · today" value={formatTokens(data?.usage.today ?? 0)} />
              <StatCard label="Self-hosted tokens · 30 days" value={formatTokens(data?.usage.month ?? 0)} />
              <StatCard label="Generations · 30 days" value={String(data?.usage.generations ?? 0)} />
            </div>

            {backends.map((b) => (
              <Panel key={b.id}>
                <div className="mb-4 flex items-center gap-3">
                  <span
                    className="h-2 w-2 shrink-0 rounded-full"
                    style={{ background: b.health.ok ? 'var(--theme-success)' : 'var(--theme-danger)' }}
                  />
                  <span className="text-sm font-semibold text-fg">{b.name}</span>
                  <span className="min-w-0 truncate text-xs text-muted">{b.baseUrl}</span>
                  <span className="ml-auto shrink-0 text-xs text-muted">
                    {b.health.ok ? `up · ${b.health.latencyMs}ms` : 'unreachable'}
                  </span>
                </div>
                {b.health.ok ? (
                  <div className="space-y-2.5">
                    <div>
                      <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">Serving now</div>
                      <div className="flex flex-wrap gap-1.5">
                        {(b.health.servingNow.length ? b.health.servingNow : ['(none reported)']).map((m) => (
                          <span key={m} className="rounded-full border border-line-subtle px-2.5 py-0.5 text-xs text-fg">
                            {m}
                          </span>
                        ))}
                      </div>
                    </div>
                  </div>
                ) : (
                  <div className="text-xs text-muted">{b.health.note}</div>
                )}
              </Panel>
            ))}

            {(data?.usage.perModel.length ?? 0) > 0 && (
              <Panel>
                <div className="mb-4 text-sm font-semibold text-fg">Served self-hosted · 30 days</div>
                <div className="divide-y divide-line-subtle">
                  {data!.usage.perModel.map((m) => (
                    <div key={m.llmModel ?? '?'} className="flex items-center gap-3 py-3 text-sm">
                      <span className="min-w-0 flex-1 truncate text-fg">{m.llmModel ?? 'unattributed'}</span>
                      <span className="shrink-0 text-muted">{formatTokens(m.tokens)} tokens</span>
                    </div>
                  ))}
                </div>
              </Panel>
            )}
          </>
        )}
      </div>
    </div>
  )
}

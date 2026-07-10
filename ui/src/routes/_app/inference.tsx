import { createFileRoute, Link } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { StatCard } from '@/components/ui/stat-card'
import { formatTokens } from '@/lib/cost'
import { useSession } from '@/lib/session'

export const Route = createFileRoute('/_app/inference')({
  component: InferencePage,
})

interface LocalBackend {
  id: string
  name: string
  baseUrl: string | null
  models: string[]
  health: { ok: boolean; latencyMs: number | null; servingNow: string[]; note: string | null }
}

interface InferenceData {
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
    refetchInterval: 30_000,
  })

// Your own hardware + on-prem: self-hosted backends probed live, plus what they've served.
// Every token here is a token you didn't buy from a cloud API.
function InferencePage() {
  const { data: session } = useSession()
  const { data, isLoading } = useInference()
  const backends = data?.backends ?? []

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-8">
        <div className="flex items-center gap-3">
          <h1 className="mercury-text text-2xl font-semibold">Self-hosted compute</h1>
          {session?.role === 'admin' && (
            <Link to="/models" className="ml-auto text-sm text-accent hover:underline">
              Configure on Models →
            </Link>
          )}
        </div>

        {isLoading ? (
          <div className="text-sm text-muted">Probing backends</div>
        ) : backends.length === 0 ? (
          <EmptyState
            icon="▦"
            title="No self-hosted backends"
            hint="Add a self-hosted provider (Ollama, vLLM, a router) on the Models page — LAN and loopback URLs are classed self-hosted automatically."
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

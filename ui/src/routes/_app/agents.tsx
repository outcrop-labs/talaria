import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Panel } from '@/components/ui/panel'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Disclosure } from '@/components/ui/disclosure'
import { Markdown } from '@/components/ui/markdown'
import { useFleet, relativeTime, STATUS_COLOR } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { importFleet, useFleetDefs, type AgentDef, type ModelTarget } from '@/lib/fleet-defs'

export const Route = createFileRoute('/_app/agents')({
  component: AgentsRoster,
})

function AgentsRoster() {
  const { data, isLoading } = useFleet()
  const agents = data?.agents ?? []
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="mx-auto max-w-5xl space-y-6">
        <h1 className="mercury-text text-2xl font-semibold">Agents</h1>

        {isAdmin && <DefinitionsPanel />}

        {isLoading ? (
          <div className="text-sm text-muted">Loading agents…</div>
        ) : (
          <Panel className="overflow-hidden">
            <table className="w-full text-sm">
              <thead className="border-b border-line bg-card2 text-xs uppercase tracking-wide text-muted">
                <tr>
                  <th className="px-4 py-2.5 text-left font-semibold">Agent</th>
                  <th className="px-4 py-2.5 text-left font-semibold">Status</th>
                  <th className="px-4 py-2.5 text-left font-semibold">Last seen</th>
                  <th className="px-4 py-2.5 text-right font-semibold">Conversations</th>
                  <th className="px-4 py-2.5 text-right font-semibold">Messages</th>
                  <th className="px-4 py-2.5 text-right font-semibold">Last used</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-line-subtle">
                {agents.map((a) => (
                  <tr key={a.id} className="transition-colors hover:bg-card">
                    <td className="px-4 py-2.5">
                      <div className="flex items-center gap-2.5">
                        <Avatar name={a.label} className="h-7 w-7" />
                        <div className="min-w-0">
                          <div className="truncate text-fg">{a.label}</div>
                          <div className="truncate text-xs text-muted">{a.role}</div>
                        </div>
                      </div>
                    </td>
                    <td className="px-4 py-2.5">
                      <span className="inline-flex items-center gap-1.5 text-xs">
                        <span className="h-2 w-2 rounded-full" style={{ background: STATUS_COLOR[a.status] }} />
                        <span className="text-muted">{a.status}</span>
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-muted">{relativeTime(a.lastSeen)}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums text-fg">{a.conversations}</td>
                    <td className="px-4 py-2.5 text-right tabular-nums text-fg">{a.messages}</td>
                    <td className="px-4 py-2.5 text-right text-muted">{relativeTime(a.lastUsed)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Panel>
        )}
      </div>
    </div>
  )
}

/** Endpoint-class chip: where a model tier runs (local plane vs cloud). */
function TargetChip({ t, name }: { t: ModelTarget; name?: string }) {
  const local = /inference|vllm|ollama|local/.test(t.endpoint)
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-line px-2 py-0.5 text-xs">
      {name && <span className="font-semibold text-fg">{name}</span>}
      <span className="text-muted">{t.model}</span>
      <span className={local ? 'text-[color:var(--theme-success)]' : 'text-accent'}>{local ? 'local' : 'cloud'}</span>
    </span>
  )
}

// The Talaria-owned agent definitions (harness phase A: imported + read-only;
// rendering/orchestration and in-app editing land next).
function DefinitionsPanel() {
  const qc = useQueryClient()
  const { data } = useFleetDefs(true)
  const defs = data?.defs ?? []
  const [busy, setBusy] = useState(false)
  const [summary, setSummary] = useState<string | null>(null)

  const runImport = async () => {
    setBusy(true)
    setSummary(null)
    try {
      const r = await importFleet()
      if (!r) return setSummary('import failed')
      const created = r.agents.filter((a) => a.created).length
      setSummary(
        `${r.agents.length} agents scanned · ${created} new version${created === 1 ? '' : 's'}` +
          (r.errors.length ? ` · ${r.errors.length} errors: ${r.errors.join('; ')}` : ''),
      )
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel className="p-4">
      <div className="mb-3 flex items-center gap-3">
        <div>
          <h2 className="text-sm font-semibold text-fg">Definitions</h2>
          <p className="text-xs text-muted">
            Talaria-owned agent configs — soul, model tiers, escalations — versioned per agent.
          </p>
        </div>
        <Button variant="outline" size="sm" className="ml-auto shrink-0" onClick={() => void runImport()} disabled={busy}>
          {busy ? 'Importing…' : defs.length ? 'Re-import from stack' : 'Import from stack'}
        </Button>
      </div>
      {summary && <div className="mb-3 text-xs text-muted">{summary}</div>}

      {defs.length === 0 ? (
        <div className="py-4 text-center text-xs text-muted">
          Nothing imported yet — pull the existing stack in to seed definitions.
        </div>
      ) : (
        <ul className="divide-y divide-line-subtle">
          {defs.map((d) => (
            <DefRow key={d.id} def={d} />
          ))}
        </ul>
      )}
    </Panel>
  )
}

function DefRow({ def: d }: { def: AgentDef }) {
  const cfg = d.latest?.config
  return (
    <li className="py-3">
      <div className="flex items-start gap-3">
        <Avatar name={d.displayName} className="mt-0.5 h-7 w-7" />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <span className="text-sm font-medium text-fg">{d.displayName}</span>
            <span className="truncate text-xs text-muted">{d.model}</span>
            <span className="ml-auto shrink-0 text-xs text-muted">
              v{d.currentVersion}
              {!d.managed && ' · imported'}
            </span>
          </div>
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            {cfg?.main && <TargetChip t={cfg.main} name="main" />}
            {cfg?.aliases?.map((a) => <TargetChip key={a.name} t={a} name={a.name} />)}
            {!!cfg?.fallbacks?.length && (
              <span className="text-xs text-muted">
                ↯ fallback: {cfg.fallbacks.map((f) => f.model).join(' → ')}
              </span>
            )}
          </div>
          {(d.latest?.soul || !!cfg?.mcpServers?.length) && (
            <div className="mt-1.5">
              <Disclosure title="Soul & tools" icon={<span>❖</span>}>
                {!!cfg?.mcpServers?.length && (
                  <div className="mb-2 text-xs text-muted">MCP: {cfg.mcpServers.join(', ')}</div>
                )}
                {!!cfg?.plugins?.length && (
                  <div className="mb-2 text-xs text-muted">Plugins: {cfg.plugins.join(', ')}</div>
                )}
                {d.latest?.soul && (
                  <div className="max-h-64 overflow-y-auto rounded-lg border border-line-subtle p-3 text-xs">
                    <Markdown>{d.latest.soul}</Markdown>
                  </div>
                )}
              </Disclosure>
            </div>
          )}
        </div>
      </div>
    </li>
  )
}

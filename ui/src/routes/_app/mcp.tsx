import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Panel } from '@/components/ui/panel'
import { useSession } from '@/lib/session'

export const Route = createFileRoute('/_app/mcp')({
  component: McpPage,
})

interface McpServerEntry {
  name: string
  url: string
  timeout: number | null
  extras: string[]
}

interface AgentMcp {
  id: string
  slug: string
  displayName: string
  managed: boolean
  servers: McpServerEntry[]
}

const useMcp = () =>
  useQuery({
    queryKey: ['mcp-agents'],
    queryFn: async (): Promise<AgentMcp[]> => {
      const r = await fetch('/api/mcp')
      if (!r.ok) throw new Error('failed to load')
      return ((await r.json()) as { agents: AgentMcp[] }).agents
    },
  })

// Each agent's MCP servers, straight from its versioned Hermes config. Edits
// append a new config version (diffable, revertible) and restart the managed
// container so the change is live.
function McpPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: agents = [], isLoading } = useMcp()

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-8">
        <h1 className="mercury-text text-2xl font-semibold">MCP</h1>

        <Panel>
          <div className="mb-2 text-sm font-semibold text-fg">Talaria's own MCP server</div>
          <p className="text-sm text-muted">
            <code className="text-xs">talaria-mcp</code> gives any agent safe board access — list, read, create into
            inbox, triage, comment, report, log time. No assign, no complete. Point a client at{' '}
            <code className="text-xs">mcp/dist/index.js</code> with <code className="text-xs">TALARIA_AGENT_KEY</code>;
            see <code className="text-xs">mcp/README.md</code>.
          </p>
        </Panel>

        {isLoading ? (
          <div className="text-sm text-muted">Loading agents…</div>
        ) : agents.length === 0 ? (
          <EmptyState icon="⧈" title="No agents yet" hint="Import your stack on the Agents page first." />
        ) : (
          agents.map((a) => <AgentCard key={a.id} agent={a} isAdmin={isAdmin} />)
        )}
      </div>
    </div>
  )
}

function AgentCard({ agent, isAdmin }: { agent: AgentMcp; isAdmin: boolean }) {
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const edit = async (body: { add?: Array<{ name: string; url: string }>; remove?: string[] }) => {
    setBusy(true)
    setErr(null)
    try {
      const r = await fetch(`/api/fleet/defs/${agent.id}/mcp`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ add: [], remove: [], apply: true, ...body }),
      }).catch(() => null)
      const j = (await r?.json().catch(() => null)) as { error?: string } | null
      if (!r?.ok || j?.error) setErr(j?.error ?? 'edit failed')
      else {
        setName('')
        setUrl('')
        await qc.invalidateQueries({ queryKey: ['mcp-agents'] })
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel>
      <div className="mb-4 flex items-center gap-3">
        <span className="text-sm font-semibold text-fg">{agent.displayName}</span>
        <span className="text-xs text-muted">
          {agent.servers.length || 'no'} server{agent.servers.length === 1 ? '' : 's'}
          {agent.managed ? ' · edits apply live (new config version + restart)' : ' · unmanaged — edits version the config only'}
        </span>
      </div>

      {agent.servers.length > 0 && (
        <div className="divide-y divide-line-subtle">
          {agent.servers.map((s) => (
            <div key={s.name} className="flex items-center gap-3 py-3 text-sm">
              <span className="w-28 shrink-0 truncate font-medium text-fg">{s.name}</span>
              <span className="min-w-0 flex-1 truncate text-muted">{s.url}</span>
              {s.timeout && <span className="shrink-0 text-xs text-muted">{s.timeout}s</span>}
              {s.extras.filter((e) => e !== 'headers').length > 0 && (
                <span className="shrink-0 text-xs text-muted">+{s.extras.filter((e) => e !== 'headers').join(', ')}</span>
              )}
              {isAdmin && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="shrink-0"
                  disabled={busy}
                  onClick={() => {
                    if (confirm(`Remove "${s.name}" from ${agent.displayName}?`)) void edit({ remove: [s.name] })
                  }}
                >
                  Remove
                </Button>
              )}
            </div>
          ))}
        </div>
      )}

      {isAdmin && (
        <div className="mt-4 flex items-center gap-2">
          <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="name" className="w-32" />
          <Input size="sm" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://host:port/mcp" className="flex-1" />
          <Button size="sm" disabled={busy || !name.trim() || !url.trim()} onClick={() => void edit({ add: [{ name: name.trim(), url: url.trim() }] })}>
            {busy ? 'Working…' : 'Add'}
          </Button>
        </div>
      )}
      {err && (
        <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
          {err}
        </div>
      )}
    </Panel>
  )
}

import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plug, Loader2, Check, Lock, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
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
type ProbeState = 'ok' | 'auth' | 'unreachable' | 'error'
interface Probe {
  state: ProbeState
  detail: string
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

const testConnection = async (url: string, agentSlug?: string): Promise<Probe> => {
  const r = await fetch('/api/mcp/test', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ url, agentSlug }),
  }).catch(() => null)
  if (!r?.ok) return { state: 'error', detail: 'test failed' }
  return r.json()
}

const STATE_UI: Record<ProbeState, { color: string; icon: React.ReactNode; label: string }> = {
  ok: { color: 'var(--theme-success)', icon: <Check size={13} />, label: 'Connected' },
  auth: { color: 'var(--theme-warning)', icon: <Lock size={13} />, label: 'Login required' },
  unreachable: { color: 'var(--theme-danger)', icon: <X size={13} />, label: 'Unreachable' },
  error: { color: 'var(--theme-danger)', icon: <X size={13} />, label: 'Error' },
}

// Each agent's MCP servers, from its versioned Hermes config. Add/remove append
// a new config version (revertible) and restart the managed container. Every
// connection can be tested live before you trust it.
function McpPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: agents = [], isLoading } = useMcp()

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-6">
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
          <EmptyState icon="⧈" title="No agents yet" hint="Import agents on the Agents page first." />
        ) : (
          agents.map((a) => <AgentCard key={a.id} agent={a} isAdmin={isAdmin} />)
        )}
      </div>
    </div>
  )
}

function AgentCard({ agent, isAdmin }: { agent: AgentMcp; isAdmin: boolean }) {
  const qc = useQueryClient()
  const [adding, setAdding] = useState(false)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [probes, setProbes] = useState<Record<string, Probe | 'testing'>>({})

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
      else await qc.invalidateQueries({ queryKey: ['mcp-agents'] })
    } finally {
      setBusy(false)
    }
  }

  const test = async (s: McpServerEntry) => {
    setProbes((p) => ({ ...p, [s.name]: 'testing' }))
    const res = await testConnection(s.url, agent.slug)
    setProbes((p) => ({ ...p, [s.name]: res }))
  }
  const testAll = () => agent.servers.forEach((s) => void test(s))

  return (
    <Panel>
      <div className="mb-4 flex items-center gap-3">
        <span className="text-sm font-semibold text-fg">{agent.displayName}</span>
        <span className="text-xs text-muted">
          {agent.servers.length || 'no'} server{agent.servers.length === 1 ? '' : 's'}
          {agent.managed ? ' · edits apply live' : ' · unmanaged (versions only)'}
        </span>
        <span className="ml-auto flex gap-2">
          {agent.servers.length > 0 && (
            <Button variant="ghost" size="sm" onClick={testAll}>
              Test all
            </Button>
          )}
          {isAdmin && (
            <Button size="sm" onClick={() => setAdding(true)}>
              <Plug size={14} className="mr-1.5" /> Add server
            </Button>
          )}
        </span>
      </div>

      {agent.servers.length > 0 ? (
        <div className="divide-y divide-line-subtle">
          {agent.servers.map((s) => {
            const probe = probes[s.name]
            return (
              <div key={s.name} className="flex items-center gap-3 py-2.5 text-sm">
                <span className="w-28 shrink-0 truncate font-medium text-fg">{s.name}</span>
                <span className="min-w-0 flex-1 truncate text-muted">{s.url}</span>
                {probe === 'testing' ? (
                  <span className="flex shrink-0 items-center gap-1 text-xs text-muted">
                    <Loader2 size={13} className="animate-spin" /> testing…
                  </span>
                ) : probe ? (
                  <span
                    className="flex shrink-0 items-center gap-1 text-xs"
                    style={{ color: STATE_UI[probe.state].color }}
                    title={probe.detail}
                  >
                    {STATE_UI[probe.state].icon} {STATE_UI[probe.state].label}
                  </span>
                ) : null}
                <button
                  type="button"
                  onClick={() => void test(s)}
                  className="shrink-0 text-xs text-muted transition-colors hover:text-accent"
                >
                  Test
                </button>
                {isAdmin && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => confirm(`Remove "${s.name}" from ${agent.displayName}?`) && void edit({ remove: [s.name] })}
                    className="shrink-0 text-xs text-muted transition-colors hover:text-[color:var(--theme-danger)]"
                  >
                    Remove
                  </button>
                )}
              </div>
            )
          })}
        </div>
      ) : (
        <div className="py-2 text-xs text-muted">No MCP servers connected.</div>
      )}
      {err && (
        <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
          {err}
        </div>
      )}

      {adding && (
        <AddServerModal
          agentSlug={agent.slug}
          onClose={() => setAdding(false)}
          onAdd={async (name, url) => {
            await edit({ add: [{ name, url }] })
            setAdding(false)
          }}
        />
      )}
    </Panel>
  )
}

function AddServerModal({
  agentSlug,
  onClose,
  onAdd,
}: {
  agentSlug: string
  onClose: () => void
  onAdd: (name: string, url: string) => Promise<void>
}) {
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [probe, setProbe] = useState<Probe | 'testing' | null>(null)
  const [busy, setBusy] = useState(false)
  const valid = name.trim() && /^https?:\/\//.test(url.trim())

  const test = async () => {
    setProbe('testing')
    setProbe(await testConnection(url.trim(), agentSlug))
  }

  return (
    <Modal open onClose={onClose} title="Add an MCP server" width="max-w-lg">
      <div className="space-y-4">
        <p className="text-xs text-muted">
          Connect a tool server to <span className="text-fg">{agentSlug}</span>. Test the connection before saving —
          Talaria sends the agent's identity header so servers that scope by agent see the right caller.
        </p>
        <div className="grid grid-cols-3 gap-2">
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
            <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="mail" />
          </div>
          <div className="col-span-2">
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">URL</label>
            <Input size="sm" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://host:port/mcp" />
          </div>
        </div>

        {probe && probe !== 'testing' && (
          <div
            className="flex items-center gap-2 rounded-lg border border-line-subtle px-3 py-2 text-xs"
            style={{ color: STATE_UI[probe.state].color }}
          >
            {STATE_UI[probe.state].icon}
            <span className="font-medium">{STATE_UI[probe.state].label}</span>
            <span className="text-muted">· {probe.detail}</span>
            {probe.state === 'auth' && (
              <span className="ml-auto text-muted">You can still add it — sign-in happens on the server.</span>
            )}
          </div>
        )}

        <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
          <Button variant="ghost" size="sm" onClick={() => void test()} disabled={!valid || probe === 'testing'}>
            {probe === 'testing' ? (
              <>
                <Loader2 size={14} className="mr-1.5 animate-spin" /> Testing…
              </>
            ) : (
              'Test connection'
            )}
          </Button>
          <span className="ml-auto" />
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            disabled={!valid || busy}
            onClick={async () => {
              setBusy(true)
              try {
                await onAdd(name.trim(), url.trim())
              } finally {
                setBusy(false)
              }
            }}
          >
            {busy ? 'Adding…' : 'Add server'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

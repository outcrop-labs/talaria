import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { Markdown } from '@/components/ui/markdown'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { useSession } from '@/lib/session'

export const Route = createFileRoute('/_app/memory')({
  component: MemoryPage,
})

interface McpAgent {
  id: string
  slug: string
  displayName: string
  managed: boolean
}

// The /api/mcp list doubles as a lightweight agent roster (id/slug/managed)
// available to every signed-in user.
const useAgentRoster = () =>
  useQuery({
    queryKey: ['mcp-agents'],
    queryFn: async (): Promise<McpAgent[]> => {
      const r = await fetch('/api/mcp')
      if (!r.ok) throw new Error('failed to load agents')
      return ((await r.json()) as { agents: McpAgent[] }).agents
    },
  })

// Each agent curates its own MEMORY.md inside its state volume; Talaria reads
// and writes it through the running container so there's no copy to drift.
function MemoryPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: agents = [], isLoading } = useAgentRoster()
  const managed = agents.filter((a) => a.managed)
  const [picked, setPicked] = useState<string | null>(null)
  const agent = managed.find((a) => a.id === picked) ?? managed[0]

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-8">
        <h1 className="mercury-text text-2xl font-semibold">Memory</h1>

        {isLoading ? (
          <div className="text-sm text-muted">Loading agents…</div>
        ) : managed.length === 0 ? (
          <EmptyState icon="❖" title="No managed agents" hint="Memory reads through the managed containers — migrate agents on /agents first." />
        ) : (
          <div className="flex items-start gap-6">
            <div className="w-52 shrink-0 space-y-1">
              {managed.map((a) => (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => setPicked(a.id)}
                  className={cn(
                    'w-full truncate rounded-lg px-3 py-2 text-left text-sm transition-colors',
                    agent?.id === a.id ? 'bg-card text-fg' : 'text-muted hover:text-fg',
                  )}
                >
                  {a.displayName}
                </button>
              ))}
            </div>
            <div className="min-w-0 flex-1">
              {agent && <MemoryEditor key={agent.id} agentId={agent.id} label={agent.displayName} isAdmin={isAdmin} />}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

function MemoryEditor({ agentId, label, isAdmin }: { agentId: string; label: string; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data, error, isLoading } = useQuery({
    queryKey: ['memory', agentId],
    queryFn: async (): Promise<{ content: string; container: string }> => {
      const r = await fetch(`/api/memory/${agentId}`)
      const j = (await r.json()) as { content?: string; container?: string; error?: string }
      if (!r.ok || j.error) throw new Error(j.error ?? 'failed to read memory')
      return { content: j.content ?? '', container: j.container ?? '' }
    },
    retry: false,
  })
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)

  const save = async (content: string) => {
    setBusy(true)
    try {
      const r = await fetch(`/api/memory/${agentId}`, {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content }),
      })
      const j = (await r.json()) as { error?: string }
      if (!j.error) await qc.invalidateQueries({ queryKey: ['memory', agentId] })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel>
      <div className="mb-4 flex items-center gap-3">
        <span className="text-sm font-semibold text-fg">{label} · MEMORY.md</span>
        {data?.container && <span className="min-w-0 truncate text-xs text-muted">{data.container} · the agent edits this too — last writer wins</span>}
        {isAdmin && !isLoading && !error && (
          <Button size="sm" className="ml-auto shrink-0" onClick={() => setEditing(true)}>
            Edit
          </Button>
        )}
      </div>
      {isLoading ? (
        <div className="text-sm text-muted">Reading memory…</div>
      ) : error ? (
        <EmptyState icon="❖" title="Can't reach the agent" hint={(error as Error).message} />
      ) : data?.content ? (
        <div className="max-h-[28rem] overflow-y-auto text-sm">
          <Markdown>{data.content}</Markdown>
        </div>
      ) : (
        <EmptyState icon="❖" title="No memory yet" hint="The agent hasn't written anything down." />
      )}

      {editing && (
        <InternalEditorModal
          open
          onClose={() => setEditing(false)}
          title={`${label} · MEMORY.md`}
          subtitle="The agent maintains this itself; your edits are snapshotted and revertible."
          value={data?.content ?? ''}
          editable={isAdmin}
          saving={busy}
          onSave={save}
          history={{ kind: 'memory', id: agentId }}
          muse={{ kind: 'memory', context: `The memory of the "${label}" agent.` }}
        />
      )}
    </Panel>
  )
}

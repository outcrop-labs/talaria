import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Panel } from '@/components/ui/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Combobox } from '@/components/ui/combobox'
import { useAgents } from '@/lib/agents'
import { useUsers } from '@/lib/users'

interface Binding {
  principalType: 'all' | 'user' | 'agent'
  principalId: string | null
}
interface RagCollection {
  id: string
  name: string
  kind: 'activity' | 'org-kb' | 'custom'
  description: string | null
  auto: boolean
  bindings: Binding[]
}

const useCollections = () =>
  useQuery({
    queryKey: ['rag-collections'],
    queryFn: async (): Promise<RagCollection[]> => {
      const r = await fetch('/api/rag/collections')
      if (!r.ok) return []
      return ((await r.json()) as { collections: RagCollection[] }).collections
    },
  })

// Admin governance of the RAG registry: the two auto brains (activity + org-kb)
// plus custom (departmental) collections, each bound to who can search it.
export function RetrievalPanel() {
  const qc = useQueryClient()
  const { data: collections = [] } = useCollections()
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)

  const create = async () => {
    if (!name.trim()) return
    setBusy(true)
    try {
      await fetch('/api/rag/collections', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name: name.trim() }) })
      setName('')
      await qc.invalidateQueries({ queryKey: ['rag-collections'] })
    } finally {
      setBusy(false)
    }
  }

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Retrieval collections</div>
      <p className="mb-4 text-xs text-muted">
        The org's RAG brains. <span className="text-fg">Workspace activity</span> and{' '}
        <span className="text-fg">Organization knowledge</span> are automatic. Spin up more for a domain or
        department, and bind each to who should search it.
      </p>
      <div className="space-y-3">
        {collections.map((c) => (
          <CollectionRow key={c.id} col={c} />
        ))}
      </div>
      <div className="mt-4 flex items-center gap-2 border-t border-line-subtle pt-3">
        <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="New collection (e.g. Sales playbook)" className="flex-1" onKeyDown={(e) => e.key === 'Enter' && void create()} />
        <Button size="sm" onClick={() => void create()} disabled={busy || !name.trim()}>
          Create
        </Button>
      </div>
    </Panel>
  )
}

function CollectionRow({ col }: { col: RagCollection }) {
  const qc = useQueryClient()
  const { data: fleet } = useAgents()
  const { data: users = [] } = useUsers()
  const bindingsAll = col.bindings.some((b) => b.principalType === 'all')
  const boundUsers = col.bindings.filter((b) => b.principalType === 'user').map((b) => b.principalId!).filter(Boolean)
  const boundAgents = col.bindings.filter((b) => b.principalType === 'agent').map((b) => b.principalId!).filter(Boolean)

  const setBindings = async (bindings: Binding[]) => {
    await fetch(`/api/rag/collections/${col.id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ bindings }) })
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }
  const del = async () => {
    if (!confirm(`Delete the "${col.name}" collection and its index?`)) return
    await fetch(`/api/rag/collections/${col.id}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }

  const applyBindings = (opts: { all?: boolean; users?: string[]; agents?: string[] }) => {
    const all = opts.all ?? bindingsAll
    const u = opts.users ?? boundUsers
    const a = opts.agents ?? boundAgents
    const next: Binding[] = []
    if (all) next.push({ principalType: 'all', principalId: null })
    for (const id of u) next.push({ principalType: 'user', principalId: id })
    for (const id of a) next.push({ principalType: 'agent', principalId: id })
    void setBindings(next)
  }

  return (
    <div className="rounded-xl border border-line-subtle p-3">
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium text-fg">{col.name}</span>
        <span className="rounded border border-line-subtle px-1.5 text-[10px] uppercase tracking-wide text-muted">{col.kind}</span>
        {col.auto && <span className="text-[10px] text-muted">auto</span>}
        <span className="ml-auto" />
        {!col.auto && (
          <button type="button" onClick={() => void del()} className="text-xs text-muted hover:text-[color:var(--theme-danger)]">
            Delete
          </button>
        )}
      </div>
      {col.description && <div className="mt-0.5 text-xs text-muted">{col.description}</div>}
      {/* Auto collections have fixed access (activity = your own scope; org-kb =
          everyone). Custom collections are bound here. */}
      {!col.auto && (
        <div className="mt-2.5 space-y-2">
          <label className="flex items-center gap-1.5 text-xs text-muted">
            <input type="checkbox" checked={bindingsAll} onChange={(e) => applyBindings({ all: e.target.checked })} className="accent-[var(--theme-accent)]" />
            Everyone
          </label>
          {!bindingsAll && (
            <div className="flex flex-col gap-2 sm:flex-row">
              <Combobox
                options={users.map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id }))}
                selected={boundUsers}
                onChange={(ids) => applyBindings({ users: ids })}
                multiple
                size="sm"
                placeholder="Bind users"
                className="min-w-0 flex-1"
              />
              <Combobox
                options={(fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label }))}
                selected={boundAgents}
                onChange={(ids) => applyBindings({ agents: ids })}
                multiple
                size="sm"
                placeholder="Bind agents"
                className="min-w-0 flex-1"
              />
            </div>
          )}
        </div>
      )}
    </div>
  )
}

import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { Panel } from '@/components/ui/panel'
import { confirm } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Combobox } from '@/components/ui/combobox'
import { useAgents } from '@/lib/agents'
import { useUsers } from '@/lib/users'
import { useTeams } from '@/lib/teams'

interface Binding {
  principalType: 'all' | 'user' | 'agent' | 'team'
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
interface RerankProviderMeta {
  id: string
  label: string
  country: string
  needsUrl: boolean
  needsKey: boolean
  liveCatalog: boolean
  fallbackModels: string[]
}
interface RagAdmin {
  health: { qdrant: boolean; embeddings: boolean }
  backfill: { state: 'idle' | 'running' | 'done' | 'error'; counts?: Record<string, number>; error?: string; finishedAt?: string }
  rerank: { providers: RerankProviderMeta[]; config: { provider: string; url?: string; model?: string; hasKey: boolean; candidates?: number } }
  spaces: Array<{ id: string; name: string; collectionId: string | null }>
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

const useRagAdmin = () =>
  useQuery({
    queryKey: ['rag-admin'],
    queryFn: async (): Promise<RagAdmin> => {
      const r = await fetch('/api/admin/rag')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    refetchInterval: (q) => (q.state.data?.backfill.state === 'running' ? 3_000 : false),
  })

// Admin governance of the RAG plane: service health + backfill, the brains
// (auto + custom collections with who-can-search bindings), which KB spaces
// feed which brain, and the reranker provider.
export function RetrievalPanel() {
  const qc = useQueryClient()
  const { data: collections = [] } = useCollections()
  const { data: rag } = useRagAdmin()
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
  const backfill = async () => {
    await fetch('/api/admin/rag', { method: 'POST' })
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Retrieval</div>
      <p className="mb-3 text-xs text-muted">
        The org's RAG brains. <span className="text-fg">Workspace activity</span> and{' '}
        <span className="text-fg">Organization knowledge</span> are automatic. Spin up more for a domain or
        team, bind who can search each, and point KB spaces at them to curate what they contain.
      </p>

      {/* Services + backfill */}
      {rag && (
        <div className="mb-4 flex flex-wrap items-center gap-3 rounded-xl border border-line-subtle p-3">
          <HealthDot ok={rag.health.qdrant} label="Vector store" />
          <HealthDot ok={rag.health.embeddings} label="Embeddings" />
          <span className="ml-auto text-xs text-muted">
            {rag.backfill.state === 'running' ? (
              <span className="flex items-center gap-1.5"><Loader2 size={12} className="animate-spin" /> backfilling</span>
            ) : rag.backfill.state === 'done' && rag.backfill.counts ? (
              `last backfill: ${Object.entries(rag.backfill.counts).map(([k, v]) => `${v} ${k}`).join(' · ')}`
            ) : rag.backfill.state === 'error' ? (
              <span style={{ color: 'var(--theme-danger)' }}>backfill failed: {rag.backfill.error}</span>
            ) : null}
          </span>
          <Button size="sm" variant="outline" onClick={() => void backfill()} disabled={rag.backfill.state === 'running' || !rag.health.qdrant || !rag.health.embeddings}>
            Reindex everything
          </Button>
        </div>
      )}

      <div className="space-y-3">
        {collections.map((c) => (
          <CollectionRow key={c.id} col={c} spaces={rag?.spaces ?? []} />
        ))}
      </div>
      <div className="mt-4 flex items-center gap-2 border-t border-line-subtle pt-3">
        <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="New brain (e.g. Sales playbook)" className="flex-1" onKeyDown={(e) => e.key === 'Enter' && void create()} />
        <Button size="sm" onClick={() => void create()} disabled={busy || !name.trim()}>
          Create
        </Button>
      </div>

      {rag && <RerankSection rag={rag} />}
    </Panel>
  )
}

const HealthDot = ({ ok, label }: { ok: boolean; label: string }) => (
  <span className="flex items-center gap-1.5 text-xs text-muted">
    <span className="h-2 w-2 rounded-full" style={{ background: ok ? 'var(--theme-success, #22c55e)' : 'var(--theme-danger)' }} />
    {label} {ok ? 'up' : 'down'}
  </span>
)

function CollectionRow({ col, spaces }: { col: RagCollection; spaces: Array<{ id: string; name: string; collectionId: string | null }> }) {
  const qc = useQueryClient()
  const { data: fleet } = useAgents()
  const { data: users = [] } = useUsers()
  const { data: teams = [] } = useTeams()
  const bindingsAll = col.bindings.some((b) => b.principalType === 'all')
  const boundUsers = col.bindings.filter((b) => b.principalType === 'user').map((b) => b.principalId!).filter(Boolean)
  const boundAgents = col.bindings.filter((b) => b.principalType === 'agent').map((b) => b.principalId!).filter(Boolean)
  const boundTeams = col.bindings.filter((b) => b.principalType === 'team').map((b) => b.principalId!).filter(Boolean)
  const feedingSpaces = spaces.filter((s) => s.collectionId === col.id).map((s) => s.id)

  const setBindings = async (bindings: Binding[]) => {
    await fetch(`/api/rag/collections/${col.id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ bindings }) })
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }
  const del = async () => {
    if (!(await confirm({ title: 'Delete collection', message: `Delete the "${col.name}" collection and its index?`, confirmLabel: 'Delete', danger: true }))) return
    await fetch(`/api/rag/collections/${col.id}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['rag-collections'] })
  }
  const setSpaces = async (ids: string[]) => {
    // Diff: newly picked spaces bind to this brain; removed ones unbind.
    const added = ids.filter((id) => !feedingSpaces.includes(id))
    const removed = feedingSpaces.filter((id) => !ids.includes(id))
    for (const spaceId of added) {
      await fetch('/api/admin/rag', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ spaceBrain: { spaceId, collectionId: col.id } }) })
    }
    for (const spaceId of removed) {
      await fetch('/api/admin/rag', { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ spaceBrain: { spaceId, collectionId: null } }) })
    }
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }

  const applyBindings = (opts: { all?: boolean; users?: string[]; agents?: string[]; teams?: string[] }) => {
    const all = opts.all ?? bindingsAll
    const next: Binding[] = []
    if (all) next.push({ principalType: 'all', principalId: null })
    for (const id of opts.users ?? boundUsers) next.push({ principalType: 'user', principalId: id })
    for (const id of opts.agents ?? boundAgents) next.push({ principalType: 'agent', principalId: id })
    for (const id of opts.teams ?? boundTeams) next.push({ principalType: 'team', principalId: id })
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
          everyone). Custom collections are bound + curated here. */}
      {!col.auto && (
        <div className="mt-2.5 space-y-2">
          <label className="flex items-center gap-1.5 text-xs text-muted">
            <input type="checkbox" checked={bindingsAll} onChange={(e) => applyBindings({ all: e.target.checked })} className="accent-[var(--theme-accent)]" />
            Everyone
          </label>
          {!bindingsAll && (
            <div className="flex flex-col gap-2 sm:flex-row">
              <Combobox
                options={teams.map((t) => ({ value: t.id, label: t.name }))}
                selected={boundTeams}
                onChange={(ids) => applyBindings({ teams: ids })}
                multiple
                size="sm"
                placeholder="Bind teams"
                className="min-w-0 flex-1"
              />
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
          <div className="flex items-center gap-2">
            <span className="shrink-0 text-xs text-muted">Fed by KB spaces:</span>
            <Combobox
              options={spaces.map((s) => ({ value: s.id, label: s.name }))}
              selected={feedingSpaces}
              onChange={(ids) => void setSpaces(ids)}
              multiple
              size="sm"
              placeholder="None — bind spaces to curate this brain"
              className="min-w-0 flex-1"
            />
          </div>
        </div>
      )}
    </div>
  )
}

// ── Reranker provider (the precision stage after vector recall) ─────────────
function RerankSection({ rag }: { rag: RagAdmin }) {
  const qc = useQueryClient()
  const cfg = rag.rerank.config
  const [key, setKey] = useState('')
  const [models, setModels] = useState<string[] | null>(null)
  const [savingKey, setSavingKey] = useState(false)
  const meta = rag.rerank.providers.find((p) => p.id === cfg.provider)

  // Autosave: provider / url / model apply on change. Only the API key (a
  // secret being committed) keeps an explicit save affordance.
  const apply = async (patch: Record<string, unknown>) => {
    await fetch('/api/admin/rag', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ reranker: patch }),
    })
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }
  const loadModels = async () => {
    const q = key ? `&key=${encodeURIComponent(key)}` : ''
    const r = await fetch(`/api/admin/rag?models=${cfg.provider}${q}`)
    if (r.ok) setModels(((await r.json()) as { models: string[] }).models)
  }
  const saveKey = async () => {
    setSavingKey(true)
    try {
      await apply({ apiKey: key })
      setKey('')
    } finally {
      setSavingKey(false)
    }
  }

  return (
    <div className="mt-5 border-t border-line-subtle pt-4">
      <div className="mb-1 text-sm font-semibold text-fg">Reranker</div>
      <p className="mb-3 text-xs text-muted">
        The precision stage after vector recall — a cross-encoder rescores the merged candidates so the best
        sources win. Off = plain vector order. Self-host it, or hook up a provider and set your default.
        Changes apply immediately.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Select size="sm" value={cfg.provider} onChange={(e) => { setModels(null); void apply({ provider: e.target.value, model: null }) }} className="w-52">
          <option value="off">Off (vector order)</option>
          {rag.rerank.providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label} ({p.country})
            </option>
          ))}
        </Select>
        {meta?.needsUrl && (
          <Input
            size="sm"
            defaultValue={cfg.url ?? ''}
            onBlur={(e) => e.target.value !== (cfg.url ?? '') && void apply({ url: e.target.value || null })}
            placeholder="http://localhost:8056 (TEI /rerank)"
            className="w-64"
          />
        )}
        {meta?.needsKey && (
          <>
            <Input size="sm" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={cfg.hasKey ? 'key saved — replace' : 'API key'} className="w-52" />
            {key && (
              <Button size="sm" onClick={() => void saveKey()} disabled={savingKey}>
                Save key
              </Button>
            )}
          </>
        )}
        {meta && meta.id !== 'tei' && (
          <>
            {models ? (
              <Select size="sm" value={cfg.model ?? ''} onChange={(e) => void apply({ model: e.target.value || null })} className="w-64">
                <option value="">default model</option>
                {models.map((m) => (
                  <option key={m} value={m}>{m}</option>
                ))}
              </Select>
            ) : (
              <Button size="sm" variant="outline" onClick={() => void loadModels()}>
                Load models
              </Button>
            )}
          </>
        )}
      </div>
      {cfg.provider !== 'off' && (
        <div className="mt-1.5 text-[11px] text-muted">
          Active: {cfg.provider}{cfg.model ? ` · ${cfg.model}` : ''}{cfg.hasKey ? ' · key sealed' : ''} · rescoring top {cfg.candidates ?? 30}
        </div>
      )}
    </div>
  )
}

import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Panel } from '@/components/ui/panel'
import { GeneratingBars } from '@/components/ui/generating'
import { SectionHeader } from '@/components/ui/section-header'
import { confirm } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { submitOnEnter } from '@/components/ui/control'
import { Select } from '@/components/ui/select'
import { Combobox } from '@/components/ui/combobox'
import { listQuery, QueryError, QueryState } from '@/components/ui/query-state'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { getJson, getList } from '@/lib/fetch-json'
import { useAgents } from '@/lib/agents'
import { cn } from '@/lib/cn'
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
  upgrade: {
    embed: { modelId: string; dim: number } | null
    collections: Array<{ id: string; name: string; pointsCount: number; denseDim: number | null; hybrid: boolean; dimMismatch: boolean; missing: boolean }>
    dimMismatch: boolean
    legacySchema: boolean
    needsReindex: boolean
  } | null
  reindex: { state: 'idle' | 'running' | 'done' | 'error'; phase?: 'rebuilding' | 'backfilling'; error?: string }
  rerank: { providers: RerankProviderMeta[]; config: { provider: string; url?: string; model?: string; hasKey: boolean; candidates?: number } }
  spaces: Array<{ id: string; name: string; collectionId: string | null }>
}

// GET /api/rag/collections is 200 `{ collections }` for anyone signed in (it
// narrows the payload for non-admins rather than 404ing), so a non-2xx is only
// ever a failure. It used to answer `[]` — which drew the panel with no brains
// at all and a "Create" box, i.e. an invitation to rebuild what already exists.
const useCollections = () =>
  useQuery({
    queryKey: ['rag-collections'],
    queryFn: (): Promise<RagCollection[]> => getList<RagCollection>('/api/rag/collections', 'collections'),
  })

const useRagAdmin = () =>
  useQuery({
    queryKey: ['rag-admin'],
    queryFn: (): Promise<RagAdmin> => getJson<RagAdmin>('/api/admin/rag'),
    refetchInterval: (q) =>
      q.state.data?.backfill.state === 'running' || q.state.data?.reindex.state === 'running' ? 3_000 : false,
  })

// Admin governance of the RAG plane: service health + backfill, the brains
// (auto + custom collections with who-can-search bindings), which KB spaces
// feed which brain, and the reranker provider.
export function RetrievalPanel() {
  const qc = useQueryClient()
  const collectionsQuery = useCollections()
  const ragQuery = useRagAdmin()
  const { data: rag, isPending: ragPending } = ragQuery
  const ragFailed = ragQuery.isError && rag === undefined
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
  const backfill = async (action: 'backfill' | 'reindex' = 'backfill') => {
    await fetch('/api/admin/rag', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action }) })
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }
  const rebuilding = rag?.reindex.state === 'running'

  return (
    <Panel>
      <SectionHeader
        className="mb-4"
        title="Retrieval"
        info="The org's RAG brains. Workspace activity and Organization knowledge are automatic. Spin up more for a domain or team, bind who can search each, and point KB spaces at them to curate what they contain."
      />

      {/* Services + backfill. A failure here must NOT paint the health dots
          red — "we could not ask" is not "the vector store is down". */}
      {ragPending && <Skeleton className="mb-4 h-10 w-full rounded-md" />}
      {ragFailed && (
        <QueryError
          className="mb-4"
          variant="compact"
          error={ragQuery.error}
          title="Could not load retrieval status"
          onRetry={() => void ragQuery.refetch()}
        />
      )}
      {rag && (
        <div className="mb-4 flex flex-wrap items-center gap-3 rounded-md border border-line p-3">
          <HealthDot ok={rag.health.qdrant} label="Vector store" />
          <HealthDot ok={rag.health.embeddings} label="Embeddings" />
          {rag.upgrade?.embed && (
            <span className="text-xs text-muted" title="Change it via TALARIA_EMBED_MODEL (docker/dev-compose.yml), restart the embeddings container, then rebuild here.">
              model <span className="font-mono text-[11px] text-fg">{rag.upgrade.embed.modelId}</span> · <span className="font-mono text-[11px]">{rag.upgrade.embed.dim}d</span>
            </span>
          )}
          <span className="ml-auto font-mono text-[10px] tracking-[0.05em] text-muted">
            {rebuilding ? (
              <span className="flex items-center gap-1.5"><GeneratingBars bars={3} variant="breathe" step={0.2} /> {rag.reindex.phase === 'backfilling' ? 'refilling from sources' : 'rebuilding collections'}</span>
            ) : rag.backfill.state === 'running' ? (
              <span className="flex items-center gap-1.5"><GeneratingBars bars={3} variant="breathe" step={0.2} /> backfilling</span>
            ) : rag.reindex.state === 'error' ? (
              <span className="text-danger">rebuild failed: {rag.reindex.error}</span>
            ) : rag.backfill.state === 'done' && rag.backfill.counts ? (
              `last backfill: ${Object.entries(rag.backfill.counts).map(([k, v]) => `${v} ${k}`).join(' · ')}`
            ) : rag.backfill.state === 'error' ? (
              <span className="text-danger">backfill failed: {rag.backfill.error}</span>
            ) : null}
          </span>
          <Button size="sm" variant="outline" onClick={() => void backfill()} disabled={rebuilding || rag.backfill.state === 'running' || !rag.health.qdrant || !rag.health.embeddings}>
            Backfill
          </Button>
        </div>
      )}

      {/* Rebuild banner: the embedding model changed (dims no longer match) or
          a brain predates hybrid keyword search. One button repairs both. */}
      {rag?.upgrade?.needsReindex && !rebuilding && (
        <div
          className={cn(
            'mb-4 flex flex-wrap items-center gap-3 rounded-md border p-3',
            rag.upgrade.dimMismatch ? 'border-danger' : 'border-warning',
          )}
        >
          <div className="min-w-0 flex-1 text-xs">
            <div className="font-semibold text-fg">
              {rag.upgrade.dimMismatch ? 'Embedding model changed — brains need a rebuild' : 'Hybrid keyword search available'}
            </div>
            <div className="mt-0.5 text-muted">
              {rag.upgrade.dimMismatch
                ? `${rag.upgrade.collections.filter((c) => c.dimMismatch).map((c) => c.name).join(', ')} no longer match the ${rag.upgrade.embed?.dim}d model — indexing and search against them are failing.`
                : 'These brains predate keyword+semantic search; a rebuild upgrades them so exact names, env vars, and error strings rank alongside meaning.'}{' '}
              Rebuilding recreates each brain and refills it from the workspace's own records; knowledge search runs thin until the refill finishes.
            </div>
          </div>
          <Button size="sm" onClick={() => void backfill('reindex')} disabled={!rag.health.qdrant || !rag.health.embeddings}>
            Rebuild index
          </Button>
        </div>
      )}

      <div className="space-y-3">
        <QueryState
          query={collectionsQuery}
          errorTitle="Could not load the org's brains"
          errorVariant="compact"
          skeleton={<>{[0, 1, 2].map((i) => <SkeletonCard key={i} delay={i * 0.15} />)}</>}
        >
          {(collections) => collections.map((c) => <CollectionRow key={c.id} col={c} spaces={rag?.spaces ?? []} />)}
        </QueryState>
      </div>
      {/* Creating a brain while the list is unknown risks a duplicate of one
          that is already there — hold the box until we know what exists. */}
      <div className="mt-4 flex items-center gap-2 border-t border-line-subtle pt-3">
        <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="New brain (e.g. Sales playbook)" className="flex-1" onKeyDown={(e) => e.key === 'Enter' && void create()} />
        <Button size="sm" onClick={() => void create()} disabled={busy || !name.trim() || collectionsQuery.data === undefined}>
          Create
        </Button>
      </div>

      {ragPending && (
        // Reranker placeholder: label bar + control row, same footprint.
        <div className="mt-5 border-t border-line-subtle pt-4">
          <Skeleton className="mb-3 h-3 w-24 rounded-full" />
          <div className="flex flex-wrap items-center gap-2">
            <Skeleton className="h-8 w-52" delay={0.12} />
            <Skeleton className="h-8 w-64" delay={0.24} />
          </div>
        </div>
      )}
      {rag && <RerankSection rag={rag} />}
    </Panel>
  )
}

const HealthDot = ({ ok, label }: { ok: boolean; label: string }) => (
  <span className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
    <span className={cn('h-1.5 w-1.5 rounded-full', ok ? 'bg-success' : 'bg-danger')} />
    {label} {ok ? 'up' : 'down'}
  </span>
)

function CollectionRow({ col, spaces }: { col: RagCollection; spaces: Array<{ id: string; name: string; collectionId: string | null }> }) {
  const qc = useQueryClient()
  const { data: fleet, isPending: agentsPending } = useAgents()
  // These two feed the pickers that decide WHO CAN RETRIEVE this collection.
  // Defaulted to `[]` they turned a failed directory read into a picker with no
  // options and existing grants shown as unresolved ids — an access-control
  // surface quietly reporting that the people it grants to do not exist.
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const teamsList = listQuery(useTeams(), { title: 'Could not load teams', variant: 'inline' })
  const users = usersList.rows
  const teams = teamsList.rows
  // Until the directories land the comboboxes would misrepresent the bindings
  // (zero options, nothing selected) — hold their slots with bars instead.
  const pickersPending = agentsPending || usersList.pending || teamsList.pending
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
    <div className="rounded-md border border-line p-3">
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium text-fg">{col.name}</span>
        <span className="rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{col.kind}</span>
        {col.auto && <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">auto</span>}
        <span className="ml-auto" />
        {!col.auto && (
          <button type="button" onClick={() => void del()} className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger">
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
            <input type="checkbox" checked={bindingsAll} onChange={(e) => applyBindings({ all: e.target.checked })} className="accent-accent" />
            Everyone
          </label>
          {!bindingsAll && pickersPending && (
            <div className="flex flex-col gap-2 sm:flex-row">
              {[0, 1, 2].map((i) => (
                <Skeleton key={i} className="h-8 min-w-0 flex-1" delay={i * 0.12} />
              ))}
            </div>
          )}
          {!bindingsAll && !pickersPending && (
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
          {!bindingsAll && (teamsList.notice || usersList.notice) && (
            <div className="space-y-1">
              {teamsList.notice}
              {usersList.notice}
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
              placeholder="None. Bind spaces to curate this brain"
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
    // The candidate key goes in the body — never a query string (logs).
    const r = await fetch('/api/admin/rag', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ models: cfg.provider, ...(key ? { key } : {}) }),
    })
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
      <SectionHeader className="mb-1" title="Reranker" />
      <p className="mb-3 text-xs text-muted">
        The precision stage after vector recall: a cross-encoder rescores the merged candidates so the best
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
            <Input size="sm" type="password" value={key} onChange={(e) => setKey(e.target.value)} onKeyDown={submitOnEnter(() => key && !savingKey && void saveKey())} placeholder={cfg.hasKey ? 'replace saved key' : 'API key'} className="w-52" />
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
        <div className="mt-1.5 font-mono text-[10px] tracking-[0.05em] text-muted">
          Active: {cfg.provider}{cfg.model ? ` · ${cfg.model}` : ''}{cfg.hasKey ? ' · key sealed' : ''} · rescoring top {cfg.candidates ?? 30}
        </div>
      )}
    </div>
  )
}

import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Checkbox, Toggle } from '@/components/ui/checkbox'
import { confirm } from '@/components/ui/confirm'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { submitOnEnter } from '@/components/ui/control'
import { Modal } from '@/components/ui/modal'
import { Generating } from '@/components/ui/generating'
import { Panel } from '@/components/ui/panel'
import { InfoTip } from '@/components/ui/info-tip'
import { SectionHeader } from '@/components/ui/section-header'
import { Select } from '@/components/ui/select'
import { Tabs } from '@/components/ui/tabs'
import { Skeleton, SkeletonCard, SkeletonRows } from '@/components/ui/skeleton'
import { Combobox } from '@/components/ui/combobox'
import { ProviderMark } from '@/components/fleet/provider-mark'
import { Chip } from '@/components/ui/chip'
import { popPanel, popRow } from '@/components/chat/chat-chrome'
import { cn } from '@/lib/cn'
import { useSession } from '@/lib/session'
import {
  addEndpoint,
  inferClass,
  patchEndpoint,
  removeEndpoint,
  PROVIDER_PRESETS,
  useAvailableModels,
  useEndpoints,
  type AffectedAgent,
  type EndpointOpResult,
} from '@/lib/models'
import { useModels } from '@/lib/muse'
import type { LlmEndpoint } from '@/lib/fleet-defs'

export const Route = createFileRoute('/_app/models')({
  component: ModelsPage,
  // /models?tab=roles deep-links a tab.
  validateSearch: (search: Record<string, unknown>): { tab?: ModelsTab } => {
    const t = MODEL_TABS.find((v) => v.id === search.tab)
    return t && t.id !== 'models' ? { tab: t.id } : {}
  },
})

// The model-backend registry: providers + the models each offers. Agents'
// tiers pick from these catalogs; the class (local/cloud) drives the cost split.
const MODEL_TABS = [
  { id: 'models', label: 'Models' },
  { id: 'roles', label: 'Roles' },
  { id: 'platform', label: 'Platform' },
  { id: 'access', label: 'Access' },
] as const
type ModelsTab = (typeof MODEL_TABS)[number]['id']

function ModelsPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: endpoints = [], isPending: endpointsPending } = useEndpoints(isAdmin)
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const tab: ModelsTab = search.tab ?? 'models'
  const setTab = (t: ModelsTab) => void navigate({ search: t === 'models' ? {} : { tab: t } })
  const [adding, setAdding] = useState(false)
  // A just-added provider: jump straight into its manage modal so models get
  // picked from the provider's live catalog (endpoints start with none).
  const [manageId, setManageId] = useState<string | null>(null)
  const managing = manageId ? endpoints.find((e) => e.id === manageId) : undefined

  if (session && !isAdmin) return <EmptyState icon="▤" title="Admins only" />

  const local = endpoints.filter((e) => e.class === 'local')
  const cloud = endpoints.filter((e) => e.class === 'cloud')

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-4xl space-y-6">
        <div className="flex items-center gap-3">
          <h1 className="font-sans text-2xl font-semibold tracking-tight text-fg">Models</h1>
          {tab === 'models' && (
            <Button size="sm" className="ml-auto" onClick={() => setAdding(true)}>
              Add provider
            </Button>
          )}
        </div>

        <Tabs items={MODEL_TABS} value={tab} onChange={setTab} />

        {tab === 'models' &&
          (endpointsPending ? (
            <div className="space-y-3">
              <SkeletonCard />
              <SkeletonCard delay={0.15} />
            </div>
          ) : endpoints.length === 0 ? (
            <EmptyState
              icon="▤"
              title="No model backends yet"
              hint="Add a provider, or import your stack on the Agents page to seed them."
              action={<Button size="sm" onClick={() => setAdding(true)}>Add provider</Button>}
            />
          ) : (
            <>
              <Section title="Self-hosted: your hardware & on-prem" endpoints={local} />
              <Section title="Cloud" endpoints={cloud} />
            </>
          ))}
        {tab === 'roles' && <ModelRolesPanel />}
        {tab === 'platform' && <PlatformAgentsPanel />}
        {tab === 'access' && <MemberAccessPanel />}

        {adding && <AddProviderModal open={adding} onClose={() => setAdding(false)} onAdded={setManageId} />}
        {managing && <EndpointModal ep={managing} onClose={() => setManageId(null)} />}
      </div>
    </div>
  )
}

function Section({ title, endpoints }: { title: string; endpoints: LlmEndpoint[] }) {
  if (endpoints.length === 0) return null
  // §8 section header: 10px mono uppercase ink-dim + right-aligned mono count.
  return (
    <div>
      <SectionHeader className="mb-2" title={title} action={String(endpoints.length).padStart(2, '0')} />
      <div className="space-y-3">
        {endpoints.map((e) => (
          <EndpointCard key={e.id} ep={e} />
        ))}
      </div>
    </div>
  )
}

const describeAffected = (affected: AffectedAgent[]) =>
  affected
    .map((a) => `  • ${a.slug}${a.aliases.length ? ` (tiers: ${a.aliases.join(', ')})` : ''}${a.fallbacks ? ' (fallback)' : ''}`)
    .join('\n')

// Compact provider card — identity + a model count + Manage. Everything
// detailed (the model list, pricing, privacy) lives in the modal so the main
// view stays scannable.
function EndpointCard({ ep }: { ep: LlmEndpoint }) {
  const [managing, setManaging] = useState(false)
  return (
    <>
      <Panel className="flex items-center gap-3">
        <ProviderMark provider={ep.provider} name={ep.name} />
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <span className="font-sans text-sm font-semibold text-fg">{ep.name}</span>
            <span
              className={cn(
                'shrink-0 font-mono text-[10px] uppercase tracking-[0.05em]',
                ep.class === 'local' ? 'text-success' : 'text-accent',
              )}
            >
              {ep.class === 'local' ? 'self-hosted' : 'cloud'}
            </span>
          </div>
          <div className="truncate font-mono text-[11px] text-muted">
            {ep.models.length} model{ep.models.length === 1 ? '' : 's'}
            {ep.baseUrl && ` · ${ep.baseUrl}`}
          </div>
        </div>
        <Button variant="outline" size="sm" className="shrink-0" onClick={() => setManaging(true)}>
          Manage
        </Button>
      </Panel>
      {managing && <EndpointModal ep={ep} onClose={() => setManaging(false)} />}
    </>
  )
}

// The provider's full surface: models (add/remove with catalog search), pricing,
// class, privacy routing, and removal.
function EndpointModal({ ep, onClose }: { ep: LlmEndpoint; onClose: () => void }) {
  const qc = useQueryClient()
  const { data: available } = useAvailableModels(ep.id)
  const [err, setErr] = useState<string | null>(null)
  const [key, setKey] = useState('')
  const [savingKey, setSavingKey] = useState(false)
  // Registry edits change the gateway catalog too — refresh it so the Member
  // access panel and every model picker see additions/removals immediately.
  const refresh = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: ['fleet-endpoints'] }),
      qc.invalidateQueries({ queryKey: ['gateway-models'] }),
    ])

  const saveKey = async (value: string | null) => {
    setSavingKey(true)
    try {
      await run(patchEndpoint(ep.id, { apiKey: value }))
      setKey('')
    } finally {
      setSavingKey(false)
    }
  }

  const run = async (p: Promise<{ error?: string }>) => {
    setErr(null)
    const r = await p
    if (r.error) setErr(r.error)
    await refresh()
  }
  const [cascading, setCascading] = useState(false)
  const runCascading = async (op: (force: boolean) => Promise<EndpointOpResult>) => {
    setErr(null)
    let r = await op(false)
    if (r.needsForce && r.affected) {
      const go = await confirm({
        title: 'Still in use',
        message: `Still used by:\n${describeAffected(r.affected)}\n\nRemove it from those agents too? Each gets a new config version (revertible); running managed agents restart.`,
        confirmLabel: 'Remove everywhere',
        danger: true,
      })
      if (go) {
        setCascading(true)
        try {
          r = await op(true)
        } finally {
          setCascading(false)
        }
      }
    }
    if (r.error) setErr(r.error)
    await refresh()
    void qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }

  const addModel = (id: string) => {
    if (!id.trim() || ep.models.includes(id.trim())) return
    void runCascading((force) => patchEndpoint(ep.id, { models: [...ep.models, id.trim()], force }))
  }
  const removeModel = (id: string) =>
    void runCascading((force) => patchEndpoint(ep.id, { models: ep.models.filter((m) => m !== id), force }))

  return (
    <Modal open onClose={onClose} title={`${ep.name} · ${ep.provider}`} width="max-w-2xl">
      <div className="space-y-5">
        <div className="flex items-center gap-3 font-mono text-[11px] text-muted">
          {ep.baseUrl && <span className="truncate">{ep.baseUrl}</span>}
          <span className="ml-auto" />
          {ep.provider === 'custom' ? (
            <Select value={ep.class} size="sm" onChange={(e) => void run(patchEndpoint(ep.id, { class: e.target.value as 'local' | 'cloud' }))}>
              <option value="local">self-hosted</option>
              <option value="cloud">cloud</option>
            </Select>
          ) : (
            <span className={cn('uppercase tracking-[0.05em]', ep.class === 'local' ? 'text-success' : 'text-accent')}>
              {ep.class === 'local' ? 'self-hosted' : 'cloud'}
            </span>
          )}
        </div>

        {/* Provider API key — stored encrypted at rest, never shown again */}
        <section>
          <div className="mb-2 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            <span>API key</span>
            {ep.hasKey ? (
              <span className="tracking-[0.05em] text-success">● encrypted key stored</span>
            ) : (
              <span className="tracking-[0.05em] text-muted">none stored{ep.apiKeyEnv ? `, using $${ep.apiKeyEnv}` : ''}</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Input
              type="password"
              value={key}
              onChange={(e) => setKey(e.target.value)}
              onKeyDown={submitOnEnter(() => key.trim() && !savingKey && void saveKey(key.trim()))}
              placeholder={ep.hasKey ? 'paste a new key to rotate' : 'paste the provider key'}
              autoComplete="off"
            />
            <Button size="sm" disabled={!key.trim() || savingKey} onClick={() => void saveKey(key.trim())}>
              {ep.hasKey ? 'Rotate' : 'Save'}
            </Button>
            {ep.hasKey && (
              <Button variant="ghost" size="sm" disabled={savingKey} onClick={() => void saveKey('')}>
                Clear
              </Button>
            )}
          </div>
          <InfoTip className="mt-1" text="Encrypted with the Talaria secret (AES-256-GCM). Stored in the database, never in a config file." />
        </section>

        {/* Models the org can use from this provider */}
        <section>
          <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Available models</div>
          {ep.models.length > 0 ? (
            <div className="mb-2 divide-y divide-line">
              {ep.models.map((m) => (
                <div key={m} className="flex items-center gap-2 py-1.5 text-sm transition-colors hover:bg-hover">
                  <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg">{m}</span>
                  <button type="button" onClick={() => removeModel(m)} className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger">
                    Remove
                  </button>
                </div>
              ))}
            </div>
          ) : (
            <EmptyState variant="inline" className="mb-2" title="No models added yet." />
          )}
          <ModelAdder catalog={available?.models ?? []} existing={ep.models} onAdd={addModel} />
          {available?.note && <div className="mt-1.5 text-xs text-muted">Provider catalog unavailable: {available.note}</div>}
        </section>

        {ep.class === 'cloud' && (
          <section>
            <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Pricing · $/1M tokens (in / out)</div>
            <div className="space-y-1.5">
              <div className="flex items-center gap-2 font-mono text-xs">
                <span className="min-w-0 flex-1 truncate text-muted">endpoint default (fallback)</span>
                <Input size="sm" type="number" defaultValue={ep.priceInPerMtok ?? ''} placeholder="in" className="w-20 shrink-0"
                  onBlur={(e) => { const v = e.target.value.trim(); void run(patchEndpoint(ep.id, { priceInPerMtok: v === '' ? null : Number(v) })) }} />
                <Input size="sm" type="number" defaultValue={ep.priceOutPerMtok ?? ''} placeholder="out" className="w-20 shrink-0"
                  onBlur={(e) => { const v = e.target.value.trim(); void run(patchEndpoint(ep.id, { priceOutPerMtok: v === '' ? null : Number(v) })) }} />
              </div>
              {ep.models.map((m) => {
                const p = ep.modelPrices?.[m]
                const auto = ep.autoPrices?.[m]
                const overridden = p?.in !== undefined || p?.out !== undefined
                const setPrice = (key: 'in' | 'out', raw: string) => {
                  const next = { ...(ep.modelPrices ?? {}) }
                  const entry = { ...(next[m] ?? {}) }
                  if (raw === '') delete entry[key]
                  else entry[key] = Number(raw)
                  if (entry.in === undefined && entry.out === undefined) delete next[m]
                  else next[m] = entry
                  void run(patchEndpoint(ep.id, { modelPrices: next }))
                }
                return (
                  <div key={m} className="flex items-center gap-2 font-mono text-xs">
                    <span className="min-w-0 flex-1 truncate text-fg">{m}</span>
                    {!overridden && auto && <span className="shrink-0 text-success">auto</span>}
                    {!overridden && !auto && <span className="shrink-0 text-muted">unpriced</span>}
                    <Input size="sm" type="number" defaultValue={p?.in ?? ''} placeholder={auto ? String(auto.in) : 'in'} className="w-20 shrink-0" onBlur={(e) => setPrice('in', e.target.value.trim())} />
                    <Input size="sm" type="number" defaultValue={p?.out ?? ''} placeholder={auto ? String(auto.out) : 'out'} className="w-20 shrink-0" onBlur={(e) => setPrice('out', e.target.value.trim())} />
                  </div>
                )
              })}
            </div>
          </section>
        )}

        {ep.class === 'cloud' && <PrivacyRow ep={ep} run={run} />}

        {cascading && (
          <Generating label="Removing across the fleet: new agent versions, re-render, rolling the affected agents" lines={2} />
        )}
        {err && <div className="text-xs text-danger">{err}</div>}

        <div className="flex items-center gap-2 border-t border-line pt-3">
          <Button
            variant="ghost"
            size="sm"
            onClick={async () => { if (await confirm({ title: 'Remove provider', message: `Remove the ${ep.name} provider?`, confirmLabel: 'Remove', danger: true })) void runCascading((force) => removeEndpoint(ep.id, force)).then(onClose) }}
          >
            Remove provider
          </Button>
          <span className="ml-auto" />
          <Button size="sm" onClick={onClose}>Done</Button>
        </div>
      </div>
    </Modal>
  )
}

// Add a model to a provider: browse/search its live catalog (in the provider's
// own order — OpenRouter lists newest first), or type any id (multi-model
// providers serve more than the catalog reports). NOT a label — a model.
function ModelAdder({ catalog, existing, onAdd }: { catalog: string[]; existing: string[]; onAdd: (id: string) => void }) {
  const [q, setQ] = useState('')
  const [open, setOpen] = useState(false)
  const suggestions = catalog.filter((m) => !existing.includes(m) && m.toLowerCase().includes(q.trim().toLowerCase()))
  const add = (id: string) => {
    onAdd(id)
    setQ('')
  }
  return (
    <div className="relative">
      <div className="flex items-center gap-2">
        <Input
          size="sm"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onFocus={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onKeyDown={(e) => e.key === 'Enter' && q.trim() && add(q.trim())}
          placeholder={`Add a model: browse the live catalog${catalog.length ? ` (${catalog.length})` : ''} or type an id`}
          className="flex-1"
        />
        <Button size="sm" onClick={() => q.trim() && add(q.trim())} disabled={!q.trim()}>
          Add
        </Button>
      </div>
      {open && suggestions.length > 0 && (
        <div className={cn(popPanel, 'absolute z-10 mt-1 max-h-52 w-full overflow-y-auto')}>
          {suggestions.map((m) => (
            <button
              key={m}
              type="button"
              onMouseDown={(e) => { e.preventDefault(); add(m) }}
              className={cn(popRow, 'font-mono text-xs text-muted hover:text-fg')}
            >
              {m}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

// Which models MEMBERS may pick (preferred model, muse drafting). Admins are
// never restricted; an empty allowlist means everything is open. This is how
// admins keep the expensive/powerful brains for deliberate use.
// ── Model Roles — which model handles each class of activity ────────────────
interface ModelRoleRow {
  role: string
  label: string
  hint: string
  wired: boolean
}

// ── Platform sub-agents: Talaria's own workers, one model pick each ─────────
interface PlatformAgentRow {
  id: string
  label: string
  job: string
  skills: string[]
  auto: string
  assignable: boolean
}

function PlatformAgentsPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['platform-agents'],
    queryFn: async (): Promise<{ agents: PlatformAgentRow[]; assignments: Record<string, string>; models: string[] }> => {
      const r = await fetch('/api/admin/platform-agents', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const assign = async (id: string, model: string | null) => {
    await fetch('/api/admin/platform-agents', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id, model }),
    })
    await qc.invalidateQueries({ queryKey: ['platform-agents'] })
  }
  if (!data)
    return (
      <Panel>
        <Skeleton className="mb-4 h-4 w-32 rounded-full" />
        <div className="space-y-4">
          {Array.from({ length: 6 }, (_, i) => (
            <div key={i} className="flex items-center gap-3">
              <div className="min-w-0 flex-1 space-y-1.5">
                <Skeleton className="h-3 w-36 rounded-full" delay={i * 0.1} />
                <Skeleton className="h-2.5 w-72 rounded-full" delay={i * 0.1 + 0.06} />
              </div>
              <Skeleton className="h-8 w-56 shrink-0" delay={i * 0.1} />
            </div>
          ))}
        </div>
      </Panel>
    )

  return (
    <Panel>
      <SectionHeader
        title="Platform agents"
        info="Talaria's own sub-agents — the workers behind internal jobs like distilling chats and drafting with Muse. Separate from your Hermes fleet: each has its own harness and skills, and you pick its model here. Unset = auto (each job's own sensible chain)."
      />
      <ul className="divide-y divide-line">
        {data.agents.map((a) => (
          <li key={a.id} className="flex items-center gap-3 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 font-sans text-sm text-fg">
                {a.label}
                {a.skills.map((sk) => (
                  <Chip key={sk}>{sk}</Chip>
                ))}
              </div>
              <div className="font-sans text-xs text-muted">{a.job}</div>
              <div className="font-mono text-[11px] text-muted/80">Auto: {a.auto}</div>
            </div>
            {a.assignable ? (
              <Select
                size="sm"
                className="w-56 shrink-0"
                value={data.assignments[a.id] ?? ''}
                onChange={(e) => void assign(a.id, e.target.value || null)}
              >
                <option value="">Auto</option>
                {data.models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            ) : (
              <span className="w-56 shrink-0 text-right font-mono text-[11px] text-muted" title={a.auto}>
                fixed by design
              </span>
            )}
          </li>
        ))}
      </ul>
    </Panel>
  )
}

function ModelRolesPanel() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['model-roles'],
    queryFn: async (): Promise<{ roles: ModelRoleRow[]; assignments: Record<string, string>; models: string[] }> => {
      const r = await fetch('/api/admin/model-roles', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const assign = async (role: string, model: string | null) => {
    await fetch('/api/admin/model-roles', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ role, model }),
    })
    await qc.invalidateQueries({ queryKey: ['model-roles'] })
  }
  if (!data)
    // Card skeleton matching the resolved layout: title bar + label/select rows.
    return (
      <Panel>
        <Skeleton className="mb-4 h-4 w-24 rounded-full" />
        <div className="space-y-4">
          {Array.from({ length: 6 }, (_, i) => (
            <div key={i} className="flex items-center gap-3">
              <div className="min-w-0 flex-1 space-y-1.5">
                <Skeleton className="h-3 w-40 rounded-full" delay={i * 0.1} />
                <Skeleton className="h-2.5 w-64 rounded-full" delay={i * 0.1 + 0.06} />
              </div>
              <Skeleton className="h-8 w-56 shrink-0" delay={i * 0.1} />
            </div>
          ))}
        </div>
      </Panel>
    )

  return (
    <Panel>
      <SectionHeader
        title="Model roles"
        info="Which model handles each class of activity. Unset = auto (a sensible pick from what's registered). Agents' own brains are configured per agent and unaffected."
      />
      <ul className="divide-y divide-line">
        {data.roles.map((r) => (
          <li key={r.role} className="flex items-center gap-3 py-2.5">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 font-sans text-sm text-fg">
                {r.label}
                {!r.wired && <Chip title="This slot takes effect when its surface lands.">reserved</Chip>}
              </div>
              <div className="font-sans text-xs text-muted">{r.hint}</div>
            </div>
            <Select
              size="sm"
              className="w-56 shrink-0"
              value={data.assignments[r.role] ?? ''}
              onChange={(e) => void assign(r.role, e.target.value || null)}
            >
              <option value="">Auto</option>
              {data.models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
          </li>
        ))}
      </ul>
    </Panel>
  )
}

function MemberAccessPanel() {
  const qc = useQueryClient()
  const { data: catalog, isPending: catalogPending } = useModels()
  const { data: settings, isPending: settingsPending } = useQuery({
    queryKey: ['admin-settings'],
    queryFn: async (): Promise<{ memberModels: string[] }> => {
      const r = await fetch('/api/admin/settings', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const models = (catalog?.models ?? []).filter((m) => !m.qualified)
  const rawSaved = settings?.memberModels ?? []
  // Models removed from the registry drop out of the list here (and get
  // persisted out on the next save) — the allowlist tracks reality. Guarded
  // on the catalog having loaded so a slow fetch can't wipe the selection.
  const registered = new Set(models.map((m) => m.id))
  const saved = models.length ? rawSaved.filter((id) => registered.has(id)) : rawSaved
  const pruned = rawSaved.length - saved.length
  // Restriction MODE is its own state — it can't derive from the selection,
  // or toggling it on with nothing selected could never stick.
  const [modeOverride, setModeOverride] = useState<boolean | null>(null)
  const [draft, setDraft] = useState<string[] | null>(null)
  const restricted = modeOverride ?? saved.length > 0
  const selection = draft ?? saved
  // What would be saved right now: the selection when limiting, [] when open.
  const effective = restricted ? selection : []
  const dirty = JSON.stringify([...effective].sort()) !== JSON.stringify([...rawSaved].sort())

  const save = async () => {
    await fetch('/api/admin/settings', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ memberModels: effective }),
    })
    setModeOverride(null)
    setDraft(null)
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
    await qc.invalidateQueries({ queryKey: ['gateway-models'] })
  }
  const toggle = (id: string) =>
    setDraft(selection.includes(id) ? selection.filter((x) => x !== id) : [...selection, id])

  return (
    <Panel>
      <SectionHeader
        title="Member access"
        info="Which models non-admins may pick for AI drafting and as their preferred model. Keep the expensive ones for deliberate, admin-configured use. Agents' own brains are set per agent and unaffected."
      />
      {catalogPending || settingsPending ? (
        // The restrict toggle and the list both seed from these queries — hold
        // them with skeletons so the checkbox never flips after load.
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <Skeleton className="h-4 w-4" />
            <Skeleton className="h-3 w-56 rounded-full" delay={0.12} />
          </div>
          <SkeletonRows rows={3} />
        </div>
      ) : (
      <>
      <Checkbox
        className="mb-3 gap-2 text-sm text-fg"
        checked={restricted}
        onChange={(checked) => setModeOverride(checked)}
        label="Limit members to selected models"
      />
      {restricted && (
        <div className="space-y-1 rounded-lg border border-line p-2">
          {models.map((m) => (
            <label key={m.id} className="flex cursor-pointer items-start gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-hover">
              <input
                type="checkbox"
                checked={selection.includes(m.id)}
                onChange={() => toggle(m.id)}
                className="mt-1 shrink-0 accent-accent"
              />
              <span className="min-w-0">
                <span className="block truncate font-sans text-sm text-fg">{m.label ?? m.id}</span>
                <span className="block truncate font-sans text-xs text-muted">{m.blurb || m.id}</span>
              </span>
            </label>
          ))}
          {models.length === 0 && <EmptyState variant="inline" className="px-2 py-1.5" title="No models registered yet." />}
        </div>
      )}
      <div className="mt-3 flex items-center gap-2">
        <span className="text-xs text-muted">
          {restricted
            ? selection.length === 0
              ? 'Pick at least one model members may use'
              : `${selection.length} model${selection.length === 1 ? '' : 's'} available to members`
            : 'All registered models are available to members'}
          {pruned > 0 && ` · ${pruned} unregistered model${pruned === 1 ? '' : 's'} will drop off on save`}
        </span>
        <span className="ml-auto" />
        <Button size="sm" onClick={() => void save()} disabled={!dirty || (restricted && selection.length === 0)}>
          Save
        </Button>
      </div>
      </>
      )}
    </Panel>
  )
}

// The OpenRouter no-train routing default: deny data collection and restrict
// to US providers. No provider list is stored — the gateway injects the live
// US pool from OpenRouter's provider catalog on every call (llm-gateway).
const OPENROUTER_NO_TRAIN = {
  provider: {
    data_collection: 'deny',
    allow_fallbacks: true,
  },
}
const GENERIC_NO_TRAIN = { provider: { data_collection: 'deny' } }

// Privacy routing as a SETTING (not a forced default): admins opt an endpoint
// into no-train routing, which Talaria merges into every call to that backend.
function PrivacyRow({ ep, run }: { ep: LlmEndpoint; run: (p: Promise<{ error?: string }>) => Promise<void> }) {
  const on = Boolean((ep.requestDefaults as { provider?: { data_collection?: string } } | undefined)?.provider?.data_collection === 'deny')
  const toggle = () => {
    const next = on ? {} : ep.provider === 'openrouter' ? OPENROUTER_NO_TRAIN : GENERIC_NO_TRAIN
    void run(patchEndpoint(ep.id, { requestDefaults: next }))
  }
  return (
    <div className="mt-4 flex items-start gap-2.5 border-t border-line pt-3">
      {/* The §8 toggle primitive: gold knob on a warm track when on. */}
      <Toggle checked={on} onChange={toggle} className="mt-0.5 shrink-0" />
      <div className="min-w-0">
        <div className="text-xs font-medium text-fg">No-train routing {on && <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-success">· on</span>}</div>
        <div className="font-sans text-[11px] text-muted">
          {ep.provider === 'openrouter'
            ? 'Restrict to US, no-store provider pools and deny data collection on every request.'
            : 'Send data_collection: deny with every request (honored where the provider supports it).'}
        </div>
      </div>
    </div>
  )
}

function AddProviderModal({ open, onClose, onAdded }: { open: boolean; onClose: () => void; onAdded: (id: string) => void }) {
  const qc = useQueryClient()
  const [presetKey, setPresetKey] = useState(PROVIDER_PRESETS[0]!.key)
  const preset = PROVIDER_PRESETS.find((p) => p.key === presetKey)!
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [apiKeyEnv, setApiKeyEnv] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const create = async () => {
    setErr(null)
    setBusy(true)
    try {
      const url = (preset.configurableUrl ? baseUrl.trim() || preset.baseUrl : preset.baseUrl) ?? null
      // No seed models — the endpoint starts empty and the manage modal opens
      // next, where models come from the provider's LIVE catalog.
      const r = await addEndpoint({
        name: name.trim() || preset.key,
        provider: preset.provider,
        baseUrl: url,
        // Users never pick local/cloud — known providers carry it, custom infers from the URL.
        class: preset.configurableUrl ? inferClass(url) : preset.class,
        apiKeyEnv: (apiKeyEnv.trim() || preset.apiKeyEnv) ?? null,
        apiKey: apiKey.trim() || null,
      })
      if (r.error) return setErr(r.error)
      await qc.invalidateQueries({ queryKey: ['fleet-endpoints'] })
      onClose()
      if (r.id) onAdded(r.id)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open={open} onClose={onClose} title="Add provider">
      <div className="space-y-4">
        <div>
          <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Provider</label>
          <Combobox
            options={PROVIDER_PRESETS.map((p) => ({
              value: p.key,
              label: p.label,
              sub: p.configurableUrl ? undefined : 'preconfigured',
              icon: <ProviderMark provider={p.provider} name={p.key} />,
            }))}
            selected={[presetKey]}
            onChange={([k]) => k && setPresetKey(k)}
            placeholder="Pick a provider"
          />
        </div>
        <div>
          <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
          <Input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder={preset.key} />
        </div>
        {preset.configurableUrl && (
          <div>
            <label className="mb-1 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              Base URL
              <InfoTip text="LAN and loopback hosts count as self-hosted in the cost split, inferred automatically." />
            </label>
            <Input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={preset.baseUrl ?? 'https://host/v1'} />
          </div>
        )}
        <div>
          <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">API key</label>
          <Input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="paste the provider key" autoComplete="off" />
          <InfoTip className="mt-1" text="Stored encrypted at rest (AES-256-GCM), never written to a config file or shown again." />
        </div>
        <details>
          <summary className="cursor-pointer font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors hover:text-muted">Advanced: env-var fallback</summary>
          <div className="mt-2">
            <Input value={apiKeyEnv} onChange={(e) => setApiKeyEnv(e.target.value)} placeholder={preset.apiKeyEnv ?? 'MY_PROVIDER_KEY'} />
            <InfoTip className="mt-1" text="Optional: an env-var name to read the key from if none is stored above (ops override)." />
          </div>
        </details>
        {err && <div className="text-sm text-danger">{err}</div>}
        <div className="flex justify-end gap-2 border-t border-line pt-3">
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void create()} disabled={busy}>
            {busy ? 'Adding' : 'Add provider'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

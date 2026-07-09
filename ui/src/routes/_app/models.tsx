import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Panel } from '@/components/ui/panel'
import { Select } from '@/components/ui/select'
import { Combobox } from '@/components/ui/combobox'
import { ProviderMark } from '@/components/fleet/provider-mark'
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
})

// The model-backend registry: providers + the models each offers. Agents'
// tiers pick from these catalogs; the class (local/cloud) drives the cost split.
function ModelsPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: endpoints = [] } = useEndpoints(isAdmin)
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
      <div className="mx-auto max-w-4xl space-y-8">
        <div className="flex items-center gap-3">
          <h1 className="mercury-text text-2xl font-semibold">Models</h1>
          <Button size="sm" className="ml-auto" onClick={() => setAdding(true)}>
            Add provider
          </Button>
        </div>

        {endpoints.length === 0 ? (
          <EmptyState
            icon="▤"
            title="No model backends yet"
            hint="Add a provider, or import your stack on the Agents page to seed them."
            action={<Button size="sm" onClick={() => setAdding(true)}>Add provider</Button>}
          />
        ) : (
          <>
            <Section title="Self-hosted — your hardware & on-prem" endpoints={local} />
            <Section title="Cloud" endpoints={cloud} />
            <MemberAccessPanel />
          </>
        )}

        {adding && <AddProviderModal open={adding} onClose={() => setAdding(false)} onAdded={setManageId} />}
        {managing && <EndpointModal ep={managing} onClose={() => setManageId(null)} />}
      </div>
    </div>
  )
}

function Section({ title, endpoints }: { title: string; endpoints: LlmEndpoint[] }) {
  if (endpoints.length === 0) return null
  return (
    <div>
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">{title}</h2>
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
    .map((a) => `  • ${a.slug}${a.aliases.length ? ` — tiers: ${a.aliases.join(', ')}` : ''}${a.fallbacks ? ' — fallback' : ''}`)
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
            <span className="text-sm font-semibold text-fg">{ep.name}</span>
            <span
              className="shrink-0 text-[11px]"
              style={{ color: ep.class === 'local' ? 'var(--theme-success)' : 'var(--theme-accent)' }}
            >
              {ep.class === 'local' ? 'self-hosted' : 'cloud'}
            </span>
          </div>
          <div className="truncate text-xs text-muted">
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
      if (go) r = await op(true)
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
        <div className="flex items-center gap-3 text-xs text-muted">
          {ep.baseUrl && <span className="truncate">{ep.baseUrl}</span>}
          <span className="ml-auto" />
          {ep.provider === 'custom' ? (
            <Select value={ep.class} size="sm" onChange={(e) => void run(patchEndpoint(ep.id, { class: e.target.value as 'local' | 'cloud' }))}>
              <option value="local">self-hosted</option>
              <option value="cloud">cloud</option>
            </Select>
          ) : (
            <span style={{ color: ep.class === 'local' ? 'var(--theme-success)' : 'var(--theme-accent)' }}>
              {ep.class === 'local' ? 'self-hosted' : 'cloud'}
            </span>
          )}
        </div>

        {/* Provider API key — stored encrypted at rest, never shown again */}
        <section>
          <div className="mb-2 flex items-center gap-2 text-[11px] uppercase tracking-wide text-muted">
            <span>API key</span>
            {ep.hasKey ? (
              <span style={{ color: 'var(--theme-success)' }}>● encrypted key stored</span>
            ) : (
              <span>none stored{ep.apiKeyEnv ? ` — using $${ep.apiKeyEnv}` : ''}</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <Input
              type="password"
              value={key}
              onChange={(e) => setKey(e.target.value)}
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
          <p className="mt-1 text-xs text-muted">Encrypted with the Talaria secret (AES-256-GCM). Stored in the database, never in a config file.</p>
        </section>

        {/* Models the org can use from this provider */}
        <section>
          <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">Available models</div>
          {ep.models.length > 0 ? (
            <div className="mb-2 divide-y divide-line-subtle">
              {ep.models.map((m) => (
                <div key={m} className="flex items-center gap-2 py-1.5 text-sm">
                  <span className="min-w-0 flex-1 truncate text-fg">{m}</span>
                  <button type="button" onClick={() => removeModel(m)} className="shrink-0 text-xs text-muted hover:text-[color:var(--theme-danger)]">
                    Remove
                  </button>
                </div>
              ))}
            </div>
          ) : (
            <div className="mb-2 text-xs text-muted">No models added yet.</div>
          )}
          <ModelAdder catalog={available?.models ?? []} existing={ep.models} onAdd={addModel} />
          {available?.note && <div className="mt-1.5 text-xs text-muted">Provider catalog unavailable: {available.note}</div>}
        </section>

        {ep.class === 'cloud' && (
          <section>
            <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">Pricing · $/1M tokens (in / out)</div>
            <div className="space-y-1.5">
              <div className="flex items-center gap-2 text-xs">
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
                  <div key={m} className="flex items-center gap-2 text-xs">
                    <span className="min-w-0 flex-1 truncate text-fg">{m}</span>
                    {!overridden && auto && <span className="shrink-0" style={{ color: 'var(--theme-success)' }}>auto</span>}
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

        {err && <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>{err}</div>}

        <div className="flex items-center gap-2 border-t border-line-subtle pt-3">
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
          placeholder={`Add a model — browse the live catalog${catalog.length ? ` (${catalog.length})` : ''} or type an id`}
          className="flex-1"
        />
        <Button size="sm" onClick={() => q.trim() && add(q.trim())} disabled={!q.trim()}>
          Add
        </Button>
      </div>
      {open && suggestions.length > 0 && (
        <div className="mercury-panel absolute z-10 mt-1 max-h-52 w-full overflow-y-auto rounded-xl p-1">
          {suggestions.map((m) => (
            <button
              key={m}
              type="button"
              onMouseDown={(e) => { e.preventDefault(); add(m) }}
              className="flex w-full items-center rounded-lg px-2 py-1.5 text-left text-sm text-muted transition-colors hover:bg-card hover:text-fg"
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
function MemberAccessPanel() {
  const qc = useQueryClient()
  const { data: catalog } = useModels()
  const { data: settings } = useQuery({
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
      <div className="mb-1 text-sm font-semibold text-fg">Member access</div>
      <p className="mb-3 text-xs text-muted">
        Which models non-admins may pick for AI drafting and as their preferred model. Keep the expensive or
        powerful ones for deliberate, admin-configured use — agents' own brains are set per agent and unaffected.
      </p>
      <label className="mb-3 flex cursor-pointer items-center gap-2 text-sm text-fg">
        <input
          type="checkbox"
          checked={restricted}
          onChange={(e) => setModeOverride(e.target.checked)}
          className="accent-[var(--theme-accent)]"
        />
        Limit members to selected models
      </label>
      {restricted && (
        <div className="space-y-1 rounded-xl border border-line-subtle p-2">
          {models.map((m) => (
            <label key={m.id} className="flex cursor-pointer items-start gap-2 rounded-lg px-2 py-1.5 hover:bg-card">
              <input
                type="checkbox"
                checked={selection.includes(m.id)}
                onChange={() => toggle(m.id)}
                className="mt-1 shrink-0 accent-[var(--theme-accent)]"
              />
              <span className="min-w-0">
                <span className="block truncate text-sm text-fg">{m.label ?? m.id}</span>
                <span className="block truncate text-xs text-muted">{m.blurb || m.id}</span>
              </span>
            </label>
          ))}
          {models.length === 0 && <div className="px-2 py-1.5 text-xs text-muted">No models registered yet.</div>}
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
    <div className="mt-4 flex items-start gap-2.5 border-t border-line-subtle pt-3">
      <button
        type="button"
        role="switch"
        aria-checked={on}
        onClick={toggle}
        className="mt-0.5 h-4 w-7 shrink-0 rounded-full transition-colors"
        style={{ background: on ? 'var(--theme-success)' : 'var(--theme-line)' }}
      >
        <span className="block h-3 w-3 rounded-full bg-white transition-transform" style={{ transform: on ? 'translateX(14px)' : 'translateX(2px)' }} />
      </button>
      <div className="min-w-0">
        <div className="text-xs font-medium text-fg">No-train routing {on && <span style={{ color: 'var(--theme-success)' }}>· on</span>}</div>
        <div className="text-[11px] text-muted">
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
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Provider</label>
          <Combobox
            options={PROVIDER_PRESETS.map((p) => ({
              value: p.key,
              label: p.label,
              sub: p.configurableUrl ? undefined : 'preconfigured',
              icon: <ProviderMark provider={p.provider} name={p.key} />,
            }))}
            selected={[presetKey]}
            onChange={([k]) => k && setPresetKey(k)}
            placeholder="Pick a provider…"
          />
        </div>
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={preset.key} />
        </div>
        {preset.configurableUrl && (
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Base URL</label>
            <Input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={preset.baseUrl ?? 'https://…/v1'} />
            <p className="mt-1 text-xs text-muted">
              LAN and loopback hosts count as <span style={{ color: 'var(--theme-success)' }}>self-hosted</span> in the cost split — inferred automatically.
            </p>
          </div>
        )}
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">API key</label>
          <Input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder="paste the provider key" autoComplete="off" />
          <p className="mt-1 text-xs text-muted">Stored encrypted at rest (AES-256-GCM) — never written to a config file or shown again.</p>
        </div>
        <details>
          <summary className="cursor-pointer text-[11px] uppercase tracking-wide text-muted">Advanced: env-var fallback</summary>
          <div className="mt-2">
            <Input value={apiKeyEnv} onChange={(e) => setApiKeyEnv(e.target.value)} placeholder={preset.apiKeyEnv ?? 'MY_PROVIDER_KEY'} />
            <p className="mt-1 text-xs text-muted">Optional: an env-var name to read the key from if none is stored above (ops override).</p>
          </div>
        </details>
        {err && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {err}
          </div>
        )}
        <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void create()} disabled={busy}>
            {busy ? 'Adding…' : 'Add provider'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

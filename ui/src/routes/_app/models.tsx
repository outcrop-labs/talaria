import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Panel } from '@/components/ui/panel'
import { Select } from '@/components/ui/select'
import { Combobox } from '@/components/ui/combobox'
import { LabelPicker } from '@/components/board/label-picker'
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

  if (session && !isAdmin) return <EmptyState icon="▚" title="Admins only" />

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
            icon="▚"
            title="No model backends yet"
            hint="Add a provider, or import your stack on the Agents page to seed them."
            action={<Button size="sm" onClick={() => setAdding(true)}>Add provider</Button>}
          />
        ) : (
          <>
            <Section title="Local — your hardware" endpoints={local} />
            <Section title="Cloud" endpoints={cloud} />
          </>
        )}

        {adding && <AddProviderModal open={adding} onClose={() => setAdding(false)} />}
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

function EndpointCard({ ep }: { ep: LlmEndpoint }) {
  const qc = useQueryClient()
  const { data: available } = useAvailableModels(ep.id)
  const [err, setErr] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['fleet-endpoints'] })

  const run = async (p: Promise<{ error?: string }>) => {
    setErr(null)
    const r = await p
    if (r.error) setErr(r.error)
    await refresh()
  }

  /** Double opt-in: a removal that agents depend on comes back needsForce with
   *  the blast radius; confirm cascades it (each agent gets a new version). */
  const runCascading = async (op: (force: boolean) => Promise<EndpointOpResult>) => {
    setErr(null)
    let r = await op(false)
    if (r.needsForce && r.affected) {
      const go = confirm(
        `Still used by:\n${describeAffected(r.affected)}\n\nRemove it from those agents too? Each gets a new config version (revertible); running managed agents restart.`,
      )
      if (go) r = await op(true)
    }
    if (r.error) setErr(r.error)
    await refresh()
    void qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }

  return (
    <Panel className="p-5">
      <div className="mb-2 flex items-center gap-2">
        <ProviderMark provider={ep.provider} name={ep.name} />
        <span className="text-sm font-semibold text-fg">{ep.name}</span>
        <span className="min-w-0 truncate text-xs text-muted">
          {ep.provider}
          {ep.baseUrl && ` · ${ep.baseUrl}`}
          {ep.apiKeyEnv && ` · key: $${ep.apiKeyEnv}`}
        </span>
        <span className="ml-auto" />
        {ep.provider === 'custom' ? (
          // Only custom endpoints can be ambiguous — known providers infer.
          <Select
            value={ep.class}
            size="sm"
            onChange={(e) => void run(patchEndpoint(ep.id, { class: e.target.value as 'local' | 'cloud' }))}
            className="shrink-0"
          >
            <option value="local">local</option>
            <option value="cloud">cloud</option>
          </Select>
        ) : (
          <span
            className="shrink-0 text-xs"
            style={{ color: ep.class === 'local' ? 'var(--theme-success)' : 'var(--theme-accent)' }}
          >
            {ep.class}
          </span>
        )}
        <Button
          variant="ghost"
          size="sm"
          className="shrink-0"
          onClick={() => {
            if (confirm(`Remove the ${ep.name} provider?`)) void runCascading((force) => removeEndpoint(ep.id, force))
          }}
        >
          Remove
        </Button>
      </div>
      <div className="mb-1 text-[11px] uppercase tracking-wide text-muted">Models</div>
      <LabelPicker
        value={ep.models}
        options={[...new Set([...ep.models, ...(available?.models ?? [])])]}
        onChange={(models) => void runCascading((force) => patchEndpoint(ep.id, { models, force }))}
        size="sm"
      />
      {available?.note && <div className="mt-1 text-xs text-muted">Provider catalog unavailable: {available.note}</div>}
      {ep.class === 'cloud' && (
        <div className="mt-3 flex items-center gap-2 text-xs text-muted">
          <span>Pricing $/1M tokens:</span>
          <Input
            size="sm"
            type="number"
            defaultValue={ep.priceInPerMtok ?? ''}
            placeholder="in"
            className="w-24"
            onBlur={(e) => void run(patchEndpoint(ep.id, { priceInPerMtok: e.target.value === '' ? null : Number(e.target.value) }))}
          />
          <Input
            size="sm"
            type="number"
            defaultValue={ep.priceOutPerMtok ?? ''}
            placeholder="out"
            className="w-24"
            onBlur={(e) => void run(patchEndpoint(ep.id, { priceOutPerMtok: e.target.value === '' ? null : Number(e.target.value) }))}
          />
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

function AddProviderModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const [presetKey, setPresetKey] = useState(PROVIDER_PRESETS[0]!.key)
  const preset = PROVIDER_PRESETS.find((p) => p.key === presetKey)!
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [apiKeyEnv, setApiKeyEnv] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const create = async () => {
    setErr(null)
    setBusy(true)
    try {
      const url = (preset.configurableUrl ? baseUrl.trim() || preset.baseUrl : preset.baseUrl) ?? null
      const r = await addEndpoint({
        name: name.trim() || preset.key,
        provider: preset.provider,
        baseUrl: url,
        // Users never pick local/cloud — known providers carry it, custom infers from the URL.
        class: preset.configurableUrl ? inferClass(url) : preset.class,
        apiKeyEnv: (apiKeyEnv.trim() || preset.apiKeyEnv) ?? null,
        models: preset.models,
      })
      if (r.error) return setErr(r.error)
      await qc.invalidateQueries({ queryKey: ['fleet-endpoints'] })
      onClose()
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
              LAN and loopback hosts count as <span style={{ color: 'var(--theme-success)' }}>local</span> in the cost split — inferred automatically.
            </p>
          </div>
        )}
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">API key env var</label>
          <Input value={apiKeyEnv} onChange={(e) => setApiKeyEnv(e.target.value)} placeholder={preset.apiKeyEnv ?? 'MY_PROVIDER_KEY'} />
          <p className="mt-1 text-xs text-muted">The variable name in your stack .env — Talaria never stores key values.</p>
        </div>
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

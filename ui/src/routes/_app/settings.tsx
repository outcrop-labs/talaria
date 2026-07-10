import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { confirm } from '@/components/ui/confirm'
import { Button, buttonClasses } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Combobox } from '@/components/ui/combobox'
import { useSession } from '@/lib/session'
import { relativeTime } from '@/lib/fleet'
import { savePreferredModel, useModels, usePreferredModel } from '@/lib/muse'
import { AssistantSection } from '@/components/assistant/assistant-section'

export const Route = createFileRoute('/_app/settings')({
  component: SettingsPage,
})

// Personal settings. Just the profile for now; more sections land here later.
function SettingsPage() {
  const qc = useQueryClient()
  const { data: user } = useSession()
  const [name, setName] = useState('')
  const [saved, setSaved] = useState(false)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (user) setName(user.name ?? '')
  }, [user])

  const save = async () => {
    const n = name.trim()
    if (!n || n === user?.name) return
    setBusy(true)
    try {
      const r = await fetch('/api/profile', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: n }),
      })
      if (r.ok) {
        await qc.invalidateQueries({ queryKey: ['session'] })
        await qc.invalidateQueries({ queryKey: ['users'] })
        setSaved(true)
        setTimeout(() => setSaved(false), 1500)
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mx-auto w-full max-w-lg p-8">
      <h1 className="mercury-text mb-4 text-lg font-semibold">Settings</h1>
      <section className="mercury-panel rounded-2xl p-6">
        <div className="mb-4 flex items-center gap-3">
          <Avatar src={user?.picture} name={name || user?.email} className="h-10 w-10" />
          <div className="min-w-0">
            <div className="truncate text-sm font-medium text-fg">{name || user?.email}</div>
            <div className="truncate text-xs text-muted">{user?.email}</div>
          </div>
        </div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Display name</label>
        <div className="flex items-center gap-2">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && void save()}
            placeholder="How teammates and agents see you"
          />
          <Button onClick={() => void save()} disabled={busy || !name.trim() || name.trim() === user?.name}>
            Save
          </Button>
        </div>
        {saved && <div className="mt-2 text-xs text-[color:var(--theme-success)]">Saved</div>}
        <PreferredModelPicker />
      </section>

      <AssistantSection />
      <IntegrationsSection />
      <ApiKeysSection />
    </div>
  )
}

// The model that powers AI drafting (souls, skills, memories, crons) across
// Talaria. Picks from the models the caller may use (the server filters by the
// admin's member allowlist), shown with pretty names + what each is good at —
// normal users shouldn't need to know what "qwen/qwen3-14b" means.
function PreferredModelPicker() {
  const qc = useQueryClient()
  const { data: catalog } = useModels()
  const { data: prefs } = usePreferredModel()
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Bare model names only — endpoint-qualified ids stay available as raw ids
  // for power users via the same list (they're in the catalog too).
  const models = (catalog?.models ?? []).filter((m) => !m.qualified)

  const save = async (model: string | null) => {
    setError(null)
    const r = await savePreferredModel(model)
    if (r && typeof r === 'object' && 'error' in r && r.error) setError(String(r.error))
    await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
    await qc.invalidateQueries({ queryKey: ['gateway-models'] })
    setSaved(true)
    setTimeout(() => setSaved(false), 1500)
  }

  return (
    <div className="mt-5 border-t border-line-subtle pt-4">
      <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Preferred model</label>
      <Combobox
        options={[
          {
            value: '',
            label: `Default${catalog?.effective && !prefs?.preferredModel ? ` (${catalog.effective})` : ''}`,
            sub: 'Let Talaria pick a sensible model',
          },
          ...models.map((m) => ({ value: m.id, label: m.label ?? m.id, sub: m.blurb || m.id })),
        ]}
        selected={[prefs?.preferredModel ?? '']}
        onChange={([v]) => void save(v || null)}
        placeholder="Pick a model"
      />
      <p className="mt-1 text-xs text-muted">
        Powers AI drafting across Talaria: souls, skills, memories, schedules.
        {saved && !error && <span className="ml-2 text-[color:var(--theme-success)]">Saved</span>}
        {error && <span className="ml-2" style={{ color: 'var(--theme-danger)' }}>{error}</span>}
      </p>
    </div>
  )
}

interface GoogleStatus {
  available: boolean
  connected: boolean
  email: string | null
  scope: string[]
  connectedAt: string | null
}

// Connected accounts. Per-user OAuth: connecting grants Talaria (and the agents
// working for you) offline access to build Google Docs/Sheets in YOUR Drive.
function IntegrationsSection() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['integration-google'],
    queryFn: async (): Promise<GoogleStatus> => {
      const r = await fetch('/api/integrations/google')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  // Surface the callback outcome (?google=connected|denied|) once, then clean the URL.
  const [flash, setFlash] = useState<string | null>(null)
  useEffect(() => {
    const p = new URLSearchParams(window.location.search).get('google')
    if (!p) return
    setFlash(p)
    window.history.replaceState({}, '', window.location.pathname)
    if (p === 'connected') void qc.invalidateQueries({ queryKey: ['integration-google'] })
    const t = setTimeout(() => setFlash(null), 4000)
    return () => clearTimeout(t)
  }, [qc])

  const disconnect = async () => {
    if (!(await confirm({ title: 'Disconnect Google', message: 'Disconnect Google? Agents and exports lose access to your Drive.', confirmLabel: 'Disconnect', danger: true }))) return
    await fetch('/api/integrations/google', { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['integration-google'] })
  }

  const flashText: Record<string, string> = {
    connected: 'Google account connected.',
    denied: 'Connection cancelled.',
    bad_state: 'Connection expired. Please try again.',
    exchange_failed: 'Google rejected the connection. Please try again.',
    disabled: 'Google integration is not configured.',
  }

  return (
    <section className="mercury-panel mt-6 rounded-2xl p-6">
      <div className="mb-2 text-sm font-semibold text-fg">Connected accounts</div>
      <p className="mb-4 text-xs text-muted">
        Connect Google to export docs and sheets into your Drive, and let the agents working for you build Google
        Docs on your behalf.
      </p>

      {data && !data.available ? (
        <div className="text-xs text-muted">Google integration isn’t configured on this server yet.</div>
      ) : (
        <div className="flex items-center gap-3 rounded-xl border border-line-subtle p-4">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-line-subtle text-lg">
            🗂️
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-fg">Google Drive & Docs</div>
            <div className="truncate text-xs text-muted">
              {data?.connected
                ? `Connected${data.email ? ` as ${data.email}` : ''}${data.connectedAt ? ` · ${relativeTime(data.connectedAt)}` : ''}`
                : 'Not connected'}
            </div>
          </div>
          {data?.connected ? (
            <Button variant="ghost" size="sm" onClick={() => void disconnect()}>
              Disconnect
            </Button>
          ) : (
            <a href="/api/integrations/google/connect" className={buttonClasses({ size: 'sm' })}>
              Connect
            </a>
          )}
        </div>
      )}

      {flash && (
        <div className="mt-3 text-xs" style={{ color: flash === 'connected' ? 'var(--theme-success)' : 'var(--theme-danger)' }}>
          {flashText[flash] ?? flash}
        </div>
      )}
    </section>
  )
}

interface ApiKey {
  id: string
  name: string
  prefix: string
  createdAt: string
  lastUsedAt: string | null
  revokedAt: string | null
}

// Personal keys for the Talaria LLM gateway — one org endpoint over the whole
// model stack. The secret shows exactly once at mint time.
function ApiKeysSection() {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['api-keys'],
    queryFn: async (): Promise<{ keys: ApiKey[]; canMint: boolean }> => {
      const r = await fetch('/api/keys')
      if (!r.ok) throw new Error('failed to load keys')
      return r.json()
    },
  })
  const [name, setName] = useState('')
  const [minted, setMinted] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const keys = (data?.keys ?? []).filter((k) => !k.revokedAt)
  const baseUrl = typeof window !== 'undefined' ? `${window.location.origin}/api/llm/v1` : '/api/llm/v1'

  const mint = async () => {
    setErr(null)
    const r = await fetch('/api/keys', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: name.trim() || 'default' }),
    }).catch(() => null)
    const j = (await r?.json().catch(() => null)) as { secret?: string; error?: string } | null
    if (!r?.ok || !j?.secret) return setErr(j?.error ?? 'could not mint a key')
    setMinted(j.secret)
    setName('')
    await qc.invalidateQueries({ queryKey: ['api-keys'] })
  }

  const revoke = async (id: string) => {
    if (!(await confirm({ title: 'Revoke key', message: 'Revoke this key? Anything using it stops working immediately.', confirmLabel: 'Revoke', danger: true }))) return
    await fetch(`/api/keys/${id}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['api-keys'] })
  }

  return (
    <section className="mercury-panel mt-6 rounded-2xl p-6">
      <div className="mb-2 text-sm font-semibold text-fg">API keys · Talaria LLM gateway</div>
      <p className="mb-4 text-xs text-muted">
        Connect external tools to the org's model stack: base URL <code className="text-[11px]">{baseUrl}</code>,
        any model from <code className="text-[11px]">/models</code> (or <code className="text-[11px]">endpoint/model</code> to pin a backend).
      </p>

      {keys.length > 0 && (
        <div className="mb-4 divide-y divide-line-subtle">
          {keys.map((k) => (
            <div key={k.id} className="flex items-center gap-3 py-3 text-sm">
              <span className="w-28 shrink-0 truncate font-medium text-fg">{k.name}</span>
              <code className="shrink-0 text-xs text-muted">{k.prefix}</code>
              <span className="min-w-0 flex-1 truncate text-xs text-muted">
                {k.lastUsedAt ? `used ${relativeTime(k.lastUsedAt)}` : 'never used'}
              </span>
              <Button variant="ghost" size="sm" className="shrink-0" onClick={() => void revoke(k.id)}>
                Revoke
              </Button>
            </div>
          ))}
        </div>
      )}

      {data?.canMint ? (
        <div className="flex items-center gap-2">
          <Input
            size="sm"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="key name (e.g. opencode)"
            onKeyDown={(e) => e.key === 'Enter' && void mint()}
          />
          <Button size="sm" onClick={() => void mint()}>
            Create key
          </Button>
        </div>
      ) : (
        <div className="text-xs text-muted">API keys are not enabled for your account. Ask an admin.</div>
      )}
      {minted && (
        <div className="mt-3 rounded-lg border border-line-subtle p-3">
          <div className="mb-1 text-xs text-muted">Copy it now. It won't be shown again.</div>
          <code className="break-all text-xs text-fg">{minted}</code>
        </div>
      )}
      {err && (
        <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
          {err}
        </div>
      )}
    </section>
  )
}

import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Avatar } from '@/components/ui/avatar'
import { confirm } from '@/components/ui/confirm'
import { Button, buttonClasses } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Combobox } from '@/components/ui/combobox'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { getJson, getList } from '@/lib/fetch-json'
import { useDeniedViews, useSession } from '@/lib/session'
import { relativeTime } from '@/lib/fleet'
import { savePreferredModel, useModels, usePreferredModel } from '@/lib/muse'
import { AssistantSection } from '@/components/assistant/assistant-section'
import { AppSurface } from '@/components/app/app-surface'
import { useEnabledApps } from '@/lib/apps'
import { Tabs } from '@/components/ui/tabs'
import { SectionHeader } from '@/components/ui/section-header'
import { useSavedFlash } from '@/components/ui/save-button'

export const Route = createFileRoute('/_app/settings')({
  component: SettingsPage,
  // /settings?tab=assistant deep-links a tab. `app:<slug>` tabs come from
  // enabled apps' settings surfaces — validated at render (the app list is
  // async), unknown slugs fall back to Profile.
  validateSearch: (search: Record<string, unknown>): { tab?: SettingsTab } => {
    const t = SETTINGS_TABS.find((v) => v.id === search.tab)
    if (t) return { tab: t.id }
    if (typeof search.tab === 'string' && /^app:[a-z0-9-]+$/.test(search.tab)) return { tab: search.tab as SettingsTab }
    return {}
  },
})

// Personal settings, tabbed by concern: Profile (identity + drafting model),
// Assistant (the member's whole personal agent), Connections, API keys —
// plus one tab per enabled app that ships a settings surface.

type SettingsTab = 'profile' | 'assistant' | 'connections' | 'keys' | `app:${string}`
const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: 'profile', label: 'Profile' },
  { id: 'assistant', label: 'Assistant' },
  { id: 'connections', label: 'Connections' },
  { id: 'keys', label: 'API keys' },
]
function SettingsPage() {
  const qc = useQueryClient()
  const { data: user, isLoading: sessionLoading } = useSession()
  const [name, setName] = useState('')
  const { saved, flash } = useSavedFlash()
  const [busy, setBusy] = useState(false)
  const search = Route.useSearch()
  const nav = Route.useNavigate()
  const tab: SettingsTab = search.tab ?? 'profile'
  const setTab = (t: SettingsTab) => void nav({ search: t === 'profile' ? {} : { tab: t } })
  // Enabled apps with a settings surface get their own tab, labeled by the
  // app — only for people granted the app (apps are explicit-grant).
  const { data: enabledApps = [] } = useEnabledApps()
  const deniedForApps = useDeniedViews()
  const appTabs = enabledApps
    .filter((a) => a.surfaces.settings && !deniedForApps.includes(`/x/${a.slug}`))
    .map((a) => ({ id: `app:${a.slug}` as SettingsTab, label: a.surfaces.settings! }))

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
        flash()
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto w-full max-w-2xl">
        <h1 className="mercury-text mb-4 text-lg font-semibold">Settings</h1>
        <Tabs items={[...SETTINGS_TABS, ...appTabs]} value={tab} onChange={setTab} className="mb-6" />
        {tab === 'profile' && (
        <section className="mercury-panel rounded-2xl p-6">
          <div className="mb-4 flex items-center gap-3">
            {sessionLoading ? (
              // Hold the identity header until the session lands — an empty
              // avatar/name/email that fills in later reads as a glitch.
              <>
                <Skeleton className="h-10 w-10 shrink-0 rounded-full" />
                <div className="min-w-0 flex-1 space-y-2">
                  <Skeleton className="h-3 w-36 rounded-full" delay={0.12} />
                  <Skeleton className="h-2.5 w-48 rounded-full" delay={0.24} />
                </div>
              </>
            ) : (
              <>
                <Avatar src={user?.picture} name={name || user?.email} className="h-10 w-10" />
                <div className="min-w-0">
                  <div className="truncate text-sm font-medium text-fg">{name || user?.email}</div>
                  <div className="truncate text-xs text-muted">{user?.email}</div>
                </div>
              </>
            )}
          </div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Display name</label>
          {sessionLoading ? (
            <div className="flex items-center gap-2">
              <Skeleton className="h-11 flex-1" />
              <Skeleton className="h-11 w-20" delay={0.12} />
            </div>
          ) : (
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
          )}
          {saved && <div className="mt-2 text-xs text-[color:var(--theme-success)]">Saved</div>}
          <PreferredModelPicker />
        </section>
        )}

        {tab === 'assistant' && <AssistantSection />}
        {tab === 'connections' && (
          <>
            <IntegrationsSection />
            <McpConnectionsSection />
          </>
        )}
        {tab === 'keys' && <ApiKeysSection />}
        {tab.startsWith('app:') && <AppSurface slug={tab.slice(4)} surface="settings" />}
      </div>
    </div>
  )
}

// The model that powers AI drafting (souls, skills, memories, crons) across
// Talaria. Picks from the models the caller may use (the server filters by the
// admin's member allowlist), shown with pretty names + what each is good at —
// normal users shouldn't need to know what "qwen/qwen3-14b" means.
function PreferredModelPicker() {
  const qc = useQueryClient()
  const { data: catalog, isLoading: catalogLoading } = useModels()
  const { data: prefs, isLoading: prefsLoading } = usePreferredModel()
  const { saved, flash } = useSavedFlash()
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
    flash()
  }

  return (
    <div className="mt-5 border-t border-line-subtle pt-4">
      <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Preferred model</label>
      {catalogLoading || prefsLoading ? (
        // Mounting the combobox before both queries land makes its value flip
        // from Default to the saved pick — hold with a select-shaped shimmer.
        <Skeleton className="h-11 w-full" />
      ) : (
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
      )}
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
/** Per-user MCP connected accounts: servers the org registered in per-user
 *  auth mode. Connect yours (a token header, sealed at rest) and your
 *  assistant starts carrying the server — acting as YOU there. */
interface McpConnection {
  id: string
  name: string
  label: string
  description: string | null
  requiredHeaders: Array<{ name: string; description: string | null; isSecret: boolean; placeholder: string | null }>
  authKind: 'oauth' | 'headers'
  connected: boolean
}

function McpConnectionsSection() {
  const qc = useQueryClient()
  // The section hides itself when the org runs no per-user servers. That is a
  // statement about the org, so only a REAL empty list may make it — a failed
  // read would otherwise quietly remove a connected account from view.
  const query = useQuery({
    queryKey: ['me-mcp'],
    queryFn: (): Promise<McpConnection[]> => getList<McpConnection>('/api/me/mcp', 'servers'),
  })
  const { data, isPending } = query
  const [connecting, setConnecting] = useState<string | null>(null)
  const [values, setValues] = useState<Record<string, string>>({})

  // The OAuth popup announces completion — flip to "connected" live.
  useEffect(() => {
    const onMsg = (e: MessageEvent) => {
      if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
        void qc.invalidateQueries({ queryKey: ['me-mcp'] })
      }
    }
    window.addEventListener('message', onMsg)
    return () => window.removeEventListener('message', onMsg)
  }, [qc])

  const put = async (serverId: string, headers: Record<string, string> | null) => {
    await fetch('/api/me/mcp', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ serverId, headers }),
    })
    setConnecting(null)
    setValues({})
    await qc.invalidateQueries({ queryKey: ['me-mcp'] })
  }

  if (!isPending && data?.length === 0) return null
  return (
    <section className="mercury-panel mt-4 rounded-2xl p-6">
      <SectionHeader
        title="Tool accounts"
        info="MCP servers your org runs in per-user mode. Connect your own account (stored encrypted) and your assistant can use the server as you. Disconnect any time — the server drops off your assistant on the next config render."
      />
      {isPending ? (
        <SkeletonRows rows={2} />
      ) : !data ? (
        <QueryError variant="inline" error={query.error} title="Could not load your tool accounts" onRetry={() => void query.refetch()} />
      ) : (
        <ul className="divide-y divide-line-subtle">
          {(data ?? []).map((s) => (
            <li key={s.id} className="space-y-2 py-2.5">
              <div className="flex items-center gap-3">
                <div className="min-w-0 flex-1">
                  <div className="text-sm text-fg">{s.label}</div>
                  {s.description && <div className="truncate font-sans text-xs text-muted">{s.description}</div>}
                </div>
                {s.connected ? (
                  <>
                    <span className="text-xs" style={{ color: 'var(--theme-success)' }}>connected</span>
                    <Button size="sm" variant="ghost" onClick={() => void put(s.id, null)}>
                      Disconnect
                    </Button>
                  </>
                ) : s.authKind === 'oauth' ? (
                  <Button
                    size="sm"
                    title="Sign in with the provider — your assistant then acts as you on this server"
                    onClick={() => void window.open(`/api/mcp/oauth/start?server=${s.id}&scope=me`, 'talaria-mcp-oauth', 'width=620,height=780')}
                  >
                    Connect
                  </Button>
                ) : (
                  <Button size="sm" onClick={() => setConnecting(connecting === s.id ? null : s.id)}>
                    Connect
                  </Button>
                )}
              </div>
              {connecting === s.id && (
                <div className="space-y-2 pl-1">
                  {/* Publisher-declared credentials drive the form; a server
                      without declarations falls back to one auth header. */}
                  {(s.requiredHeaders.length ? s.requiredHeaders : [{ name: 'Authorization', description: null, isSecret: true, placeholder: 'Bearer …' }]).map((h) => (
                    <div key={h.name} className="flex items-end gap-2">
                      <span className="w-40 shrink-0 truncate pb-2 text-xs text-muted" title={h.name}>
                        {h.name}
                      </span>
                      <div className="min-w-0 flex-1">
                        <Input
                          size="sm"
                          type={h.isSecret ? 'password' : 'text'}
                          value={values[h.name] ?? ''}
                          onChange={(e) => setValues((p) => ({ ...p, [h.name]: e.target.value }))}
                          placeholder={h.placeholder ?? ''}
                          autoComplete="off"
                        />
                        {h.description && <div className="mt-0.5 font-sans text-[11px] text-muted/80">{h.description}</div>}
                      </div>
                    </div>
                  ))}
                  <div className="flex justify-end">
                    <Button
                      size="sm"
                      disabled={!Object.values(values).some((v) => v.trim())}
                      onClick={() => void put(s.id, Object.fromEntries(Object.entries(values).filter(([, v]) => v.trim())))}
                    >
                      Connect
                    </Button>
                  </div>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

function IntegrationsSection() {
  const qc = useQueryClient()
  const query = useQuery({
    queryKey: ['integration-google'],
    queryFn: (): Promise<GoogleStatus> => getJson<GoogleStatus>('/api/integrations/google'),
  })
  const { data, isLoading } = query
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
    <section className="mercury-panel rounded-2xl p-6">
      <SectionHeader
        className="mb-4"
        title="Connected accounts"
        info="Connect Google to export docs and sheets into your Drive, and let the agents working for you build Google Docs on your behalf."
      />

      {isLoading ? (
        // Never show "Not connected" + Connect while the status is unknown —
        // that's a false state. Hold the row's shape until the query resolves.
        <div aria-hidden className="flex items-center gap-3 rounded-xl border border-line-subtle p-4">
          <Skeleton className="h-9 w-9 shrink-0 rounded-lg" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-3 w-40 rounded-full" delay={0.12} />
            <Skeleton className="h-2.5 w-56 rounded-full" delay={0.24} />
          </div>
          <Skeleton className="h-9 w-24 rounded-lg" delay={0.12} />
        </div>
      ) : !data ? (
        // Same reason as the skeleton above: "Not connected" is a claim about
        // the account, and a failed status read can't make it.
        <QueryError
          variant="compact"
          error={query.error}
          title="Could not load your connected accounts"
          onRetry={() => void query.refetch()}
        />
      ) : !data.available ? (
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
  const query = useQuery({
    queryKey: ['api-keys'],
    queryFn: (): Promise<{ keys: ApiKey[]; canMint: boolean }> => getJson<{ keys: ApiKey[]; canMint: boolean }>('/api/keys'),
  })
  const { data, isLoading } = query
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
    <section className="mercury-panel rounded-2xl p-6">
      <SectionHeader
        className="mb-4"
        title="API keys"
        info={`Connect external tools to the org's model stack: base URL ${baseUrl}, any model from /models (or endpoint/model to pin a backend).`}
      />

      {isLoading ? (
        // Hold BOTH the key list and the canMint branch — flashing "API keys
        // are not enabled for your account" mid-load is actively wrong.
        <div aria-hidden>
          <SkeletonRows rows={2} className="mb-4" />
          <div className="flex items-center gap-2">
            <Skeleton className="h-9 flex-1" />
            <Skeleton className="h-9 w-24" delay={0.12} />
          </div>
        </div>
      ) : !data ? (
        // The skeleton comment above holds after load too: "API keys are not
        // enabled for your account" is a permission verdict, and a failed
        // /api/keys never delivered one.
        <QueryError variant="compact" error={query.error} title="Could not load your API keys" onRetry={() => void query.refetch()} />
      ) : (
        <>
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
        </>
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

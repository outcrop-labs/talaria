import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Combobox } from '@/components/ui/combobox'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { InfoTip } from '@/components/ui/info-tip'
import { Modal } from '@/components/ui/modal'
import { Panel } from '@/components/ui/panel'
import { Select } from '@/components/ui/select'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { confirm } from '@/components/ui/confirm'
import { UserPicker } from '@/components/app/user-picker'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useAgents } from '@/lib/agents'
import { useUsers } from '@/lib/users'

// Manage → MCP: the org's MCP servers in one registry. Register a server
// once; decide which agents carry it (optionally a tool subset per agent) and
// which people may exercise it through agents acting for them (optionally
// their own tool subset). Per-user servers act with each person's CONNECTED
// ACCOUNT (Settings → Connections). Enforcement lives in the MCP gateway,
// not in agent configs.
export const Route = createFileRoute('/_app/mcp')({ component: McpPage })

interface McpServerRow {
  id: string
  name: string
  label: string
  description: string | null
  url: string
  headers: Record<string, string>
  timeoutSecs: number | null
  enabled: boolean
  allAgents: boolean
  authMode: 'org' | 'per-user'
  oauthEnabled: boolean
  /** OAuth org connection state (null for header-auth servers). */
  orgConnected: boolean | null
  oauthMeta: { dcr: boolean; clientSet: boolean } | null
  tools: Array<{ name: string; description?: string }>
  toolsRefreshedAt: string | null
  assignments: Array<{ agentModel: string; tools: string[] | null }>
  userAccess: Array<{ userId: string; allowed: boolean; tools: string[] | null }>
}

const connectPopup = (serverId: string, scope: 'org' | 'me') =>
  window.open(`/api/mcp/oauth/start?server=${serverId}&scope=${scope}`, 'talaria-mcp-oauth', 'width=620,height=780')

function useMcpServers() {
  return useQuery({
    queryKey: ['mcp-servers'],
    queryFn: async (): Promise<McpServerRow[] | null> => {
      const r = await fetch('/api/mcp/servers', { credentials: 'same-origin' })
      if (!r.ok) return null
      return ((await r.json()) as { servers: McpServerRow[] }).servers
    },
  })
}

function McpPage() {
  const qc = useQueryClient()
  const { data: servers, isPending } = useMcpServers()
  const [adding, setAdding] = useState<null | 'marketplace' | 'custom'>(null)

  // The OAuth popup announces completion — refresh connection states live.
  useEffect(() => {
    const onMsg = (e: MessageEvent) => {
      if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
        void qc.invalidateQueries({ queryKey: ['mcp-servers'] })
      }
    }
    window.addEventListener('message', onMsg)
    return () => window.removeEventListener('message', onMsg)
  }, [qc])

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex items-center gap-2">
          <h1 className="mercury-text text-2xl font-semibold">MCP</h1>
          <InfoTip text="Model Context Protocol servers, managed org-wide. Register once; choose which agents carry each server and which people may use it — down to individual tools. Agents reach servers only through Talaria's gateway, so the limits here are enforced, not advisory." />
          <span className="flex-1" />
          <Button size="sm" variant="ghost" onClick={() => setAdding('custom')}>
            Custom server
          </Button>
          <Button size="sm" onClick={() => setAdding('marketplace')}>
            <Plus size={14} /> Browse marketplace
          </Button>
        </div>

        {isPending ? (
          <Panel>
            <Skeleton className="mb-3 h-4 w-40 rounded-full" />
            <SkeletonRows rows={3} />
          </Panel>
        ) : (servers ?? []).length === 0 ? (
          <EmptyState
            icon="⌁"
            title="No MCP servers yet"
            hint="Browse the marketplace, or register a custom endpoint your agents should reach."
            action={<Button size="sm" onClick={() => setAdding('marketplace')}>Browse marketplace</Button>}
          />
        ) : (
          (servers ?? []).map((s) => <ServerCard key={s.id} server={s} />)
        )}

        {adding === 'marketplace' && <MarketplaceModal onClose={() => setAdding(null)} onCustom={() => setAdding('custom')} />}
        {adding === 'custom' && <AddServerModal onClose={() => setAdding(null)} />}
      </div>
    </div>
  )
}

async function patchServer(id: string, body: unknown): Promise<string | null> {
  const r = await fetch(`/api/mcp/servers/${id}`, {
    method: 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!r.ok) return ((await r.json().catch(() => ({}))) as { error?: string }).error ?? `failed (${r.status})`
  return null
}

function ServerCard({ server: s }: { server: McpServerRow }) {
  const qc = useQueryClient()
  const { data: fleet } = useAgents()
  const { data: users = [] } = useUsers()
  const [error, setError] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['mcp-servers'] })

  const patch = async (body: unknown) => {
    setError(null)
    const e = await patchServer(s.id, body)
    if (e) setError(e)
    await refresh()
  }

  const agentOptions = (fleet?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role }))
  const toolOptions = s.tools.map((t) => ({ value: t.name, label: t.name }))
  const userLabel = (id: string) => {
    const u = users.find((x) => x.id === id)
    return u?.name ?? u?.email ?? id.slice(0, 8)
  }

  return (
    <Panel className={cn(!s.enabled && 'opacity-60')}>
      <div className="flex items-center gap-3">
        <ServerMark title={s.label} domain={(() => { try { return new URL(s.url).hostname.replace(/^mcp\./, '') } catch { return null } })()} size={34} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold text-fg">{s.label}</span>
            <span className="rounded bg-card px-1.5 py-0.5 text-[10px] text-muted">{s.name}</span>
            {s.authMode === 'per-user' && (
              <span className="rounded bg-accent/10 px-1.5 py-0.5 text-[10px] font-medium text-accent" title="Each person connects their own account (Settings → Connections); the server acts as them.">
                per-user auth
              </span>
            )}
          </div>
          <div className="truncate font-sans text-xs text-muted">{s.url}</div>
          {s.description && <div className="truncate font-sans text-xs text-muted/80">{s.description}</div>}
        </div>
        {s.oauthEnabled && s.authMode === 'org' && (s.oauthMeta?.dcr || s.oauthMeta?.clientSet) && (
          <button
            type="button"
            onClick={() => void connectPopup(s.id, 'org')}
            className={cn(
              'flex shrink-0 items-center gap-1.5 rounded-lg border px-2.5 py-1 text-xs transition-colors',
              s.orgConnected
                ? 'border-line-subtle text-muted hover:border-line hover:text-fg'
                : 'border-accent/60 bg-accent/10 text-accent hover:bg-accent/15',
            )}
            title={
              s.orgConnected
                ? 'Connected via OAuth as the org — click to reconnect'
                : 'This server authenticates with OAuth — connect the org account so agents can use it'
            }
          >
            {s.orgConnected ? (
              <>
                <Check size={12} /> connected
              </>
            ) : (
              'Connect'
            )}
          </button>
        )}
        {s.oauthEnabled && s.authMode === 'per-user' && (
          <span className="shrink-0 text-[11px] text-muted" title="Each person connects their own account in Settings → Connections">
            OAuth · per-user
          </span>
        )}
        <Select
          size="sm"
          value={s.authMode}
          onChange={(e) => void patch({ authMode: e.target.value })}
          className="w-32 shrink-0"
          title="Org auth: shared credentials for everyone. Per-user: each person connects their own account."
        >
          <option value="org">Org auth</option>
          <option value="per-user">Per-user auth</option>
        </Select>
        <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted" title="Disabled servers vanish from every agent on the next config render">
          <input
            type="checkbox"
            checked={s.enabled}
            onChange={(e) => void patch({ enabled: e.target.checked })}
            className="accent-[var(--theme-accent)]"
          />
          enabled
        </label>
        <button
          type="button"
          title="Delete this server (assignments and connections go with it)"
          onClick={() => {
            void confirm({ title: 'Remove MCP server', message: `Remove "${s.label}" org-wide? Every agent loses it on the next config render.`, confirmLabel: 'Remove' }).then(async (ok) => {
              if (!ok) return
              await fetch(`/api/mcp/servers/${s.id}`, { method: 'DELETE', credentials: 'same-origin' })
              await refresh()
            })
          }}
          className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-muted transition-colors hover:bg-card hover:text-[color:var(--theme-danger)]"
        >
          <Trash2 size={14} />
        </button>
      </div>

      {/* DCR-less providers (GitHub): the admin registers an OAuth app with
          the provider and pastes its credentials — then Connect lights up. */}
      {s.oauthEnabled && s.oauthMeta && !s.oauthMeta.dcr && !s.oauthMeta.clientSet && (
        <OauthAppSetup serverId={s.id} domain={(() => { try { return new URL(s.url).hostname } catch { return null } })()} onSaved={refresh} />
      )}

      {/* Discovered tools */}
      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <button
          type="button"
          onClick={() => {
            setRefreshing(true)
            void patch({ refreshTools: true }).finally(() => setRefreshing(false))
          }}
          className="flex items-center gap-1.5 rounded-lg border border-line-subtle px-2 py-0.5 text-[11px] text-muted transition-colors hover:border-line hover:text-fg"
          title="Ask the server for its tool catalog"
        >
          <RefreshCw size={11} className={cn(refreshing && 'animate-spin')} />
          {s.tools.length ? `${s.tools.length} tools` : 'Discover tools'}
          {s.toolsRefreshedAt && <span className="text-muted/70">· {relativeTime(s.toolsRefreshedAt)}</span>}
        </button>
        {s.tools.slice(0, 10).map((t) => (
          <span key={t.name} title={t.description} className="rounded-full border border-line-subtle px-2 py-0.5 text-[11px] text-muted">
            {t.name}
          </span>
        ))}
        {s.tools.length > 10 && <span className="text-[11px] text-muted/70">+{s.tools.length - 10} more</span>}
      </div>

      {/* Agents */}
      <div className="mt-4 space-y-2">
        <div className="flex items-center gap-2">
          <span className="text-xs font-semibold uppercase tracking-wide text-muted">Agents</span>
          <label className="flex items-center gap-1.5 text-xs text-muted" title="Every agent carries this server (tool subsets below still apply per pick)">
            <input
              type="checkbox"
              checked={s.allAgents}
              onChange={(e) => void patch({ allAgents: e.target.checked })}
              className="accent-[var(--theme-accent)]"
            />
            all agents
          </label>
        </div>
        {!s.allAgents && (
          <div className="space-y-1.5">
            {s.assignments.map((a) => (
              <div key={a.agentModel} className="flex items-center gap-2">
                <span className="w-40 shrink-0 truncate text-sm text-fg">
                  {agentOptions.find((o) => o.value === a.agentModel)?.label ?? a.agentModel}
                </span>
                <Combobox
                  options={toolOptions}
                  selected={a.tools ?? []}
                  onChange={(tools) => void patch({ assign: { agentModel: a.agentModel, tools: tools.length ? tools : null } })}
                  multiple
                  size="sm"
                  placeholder="All tools"
                  className="min-w-0 flex-1"
                />
                <button
                  type="button"
                  title="Remove this agent"
                  onClick={() => void patch({ unassign: a.agentModel })}
                  className="grid h-6 w-6 shrink-0 place-items-center rounded text-muted hover:text-[color:var(--theme-danger)]"
                >
                  <Trash2 size={12} />
                </button>
              </div>
            ))}
            <Combobox
              options={agentOptions.filter((o) => !s.assignments.some((a) => a.agentModel === o.value))}
              selected={[]}
              onChange={(models) => {
                const m = models[0]
                if (m) void patch({ assign: { agentModel: m, tools: null } })
              }}
              multiple
              size="sm"
              placeholder="Add an agent"
              className="w-64"
            />
          </div>
        )}
      </div>

      {/* People */}
      <div className="mt-4 space-y-2">
        <div className="flex items-center gap-2">
          <span className="text-xs font-semibold uppercase tracking-wide text-muted">People</span>
          <InfoTip text="Who may exercise this server through agents acting for them (their personal assistant, for now). Nobody listed = everyone with an assigned agent. A row can deny outright or narrow to specific tools." />
        </div>
        <div className="space-y-1.5">
          {s.userAccess.map((ua) => (
            <div key={ua.userId} className="flex items-center gap-2">
              <span className="w-40 shrink-0 truncate text-sm text-fg">{userLabel(ua.userId)}</span>
              <Select
                size="sm"
                value={ua.allowed ? 'allow' : 'deny'}
                onChange={(e) => void patch({ userAccess: { userId: ua.userId, allowed: e.target.value === 'allow', tools: ua.tools } })}
                className="w-24 shrink-0"
              >
                <option value="allow">Allow</option>
                <option value="deny">Deny</option>
              </Select>
              {ua.allowed && (
                <Combobox
                  options={toolOptions}
                  selected={ua.tools ?? []}
                  onChange={(tools) => void patch({ userAccess: { userId: ua.userId, allowed: true, tools: tools.length ? tools : null } })}
                  multiple
                  size="sm"
                  placeholder="All tools"
                  className="min-w-0 flex-1"
                />
              )}
              <button
                type="button"
                title="Remove this rule (back to default access)"
                onClick={() => void patch({ userAccess: { userId: ua.userId, allowed: null, tools: null } })}
                className="grid h-6 w-6 shrink-0 place-items-center rounded text-muted hover:text-[color:var(--theme-danger)]"
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))}
          <UserPicker
            size="sm"
            className="w-64"
            placeholder="Add a person rule"
            exclude={s.userAccess.map((u) => u.userId)}
            onPick={(u) => void patch({ userAccess: { userId: u.id, allowed: true, tools: null } })}
          />
        </div>
      </div>

      {error && (
        <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
          {error}
        </div>
      )}
    </Panel>
  )
}

interface LibraryHeaderRow {
  name: string
  description: string | null
  isRequired: boolean
  isSecret: boolean
  placeholder: string | null
  default: string | null
  choices: string[] | null
}

interface LibraryServerRow {
  registryName: string
  title: string
  description: string | null
  url: string
  domain: string | null
  icon: string | null
  tier: 'first-party' | 'verified' | 'community'
  requiredHeaders: LibraryHeaderRow[]
}

/** Publisher mark: the registry-declared icon hotlinks DIRECTLY (fast, no
 *  middleman); the cached favicon proxy is only the fallback; a monogram tile
 *  is the floor. */
function ServerMark({ title, domain, icon, size = 32 }: { title: string; domain?: string | null; icon?: string | null; size?: number }) {
  const [failed, setFailed] = useState(false)
  const src = icon ?? (domain ? `/api/mcp/icon?domain=${encodeURIComponent(domain)}` : null)
  if (!src || failed) {
    return (
      <span
        className="grid shrink-0 place-items-center rounded-lg bg-card font-semibold text-muted"
        style={{ width: size, height: size, fontSize: size * 0.45 }}
      >
        {(title[0] ?? '?').toUpperCase()}
      </span>
    )
  }
  return (
    <img
      src={src}
      alt=""
      width={size}
      height={size}
      loading="lazy"
      onError={() => setFailed(true)}
      className="shrink-0 rounded-lg bg-card object-contain"
      style={{ width: size, height: size }}
    />
  )
}

const TIER_BADGE: Record<LibraryServerRow['tier'], { label: string; cls: string; hint: string }> = {
  'first-party': {
    label: 'official',
    cls: 'bg-accent/10 text-accent',
    hint: 'Published by the company itself, hosted on its own verified domain',
  },
  verified: { label: 'verified', cls: 'bg-card text-muted', hint: 'Domain-verified publisher, hosted elsewhere' },
  community: { label: 'community', cls: 'bg-card text-muted/70', hint: 'Community-built (io.github namespace)' },
}

/** The marketplace: browse the official MCP registry like a store — featured
 *  first-party servers up front, full search across 20k+ entries, one-click
 *  install. Custom/self-hosted endpoints live in the separate form. */
function MarketplaceModal({ onClose, onCustom }: { onClose: () => void; onCustom: () => void }) {
  const qc = useQueryClient()
  const { data: existing } = useMcpServers()
  const [q, setQ] = useState('')
  const [busyAdd, setBusyAdd] = useState<string | null>(null)
  const [added, setAdded] = useState<Set<string>>(new Set())
  const [error, setError] = useState<string | null>(null)
  const [installing, setInstalling] = useState<LibraryServerRow | null>(null)

  const { data: results, isFetching } = useQuery({
    queryKey: ['mcp-marketplace', q],
    queryFn: async (): Promise<{ servers: LibraryServerRow[] }> => {
      const url = q.trim()
        ? `/api/mcp/library?q=${encodeURIComponent(q.trim())}`
        : '/api/mcp/library?featured=1'
      const r = await fetch(url, { credentials: 'same-origin' })
      if (!r.ok) throw new Error('library unavailable')
      return r.json()
    },
    placeholderData: (prev) => prev,
    staleTime: 60_000,
  })

  const installedUrls = new Set((existing ?? []).map((s) => s.url))
  const slugify = (v: string) => v.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)

  const register = async (l: LibraryServerRow, opts: { headers?: Record<string, string>; authMode?: 'org' | 'per-user' }) => {
    setBusyAdd(l.registryName)
    setError(null)
    const r = await fetch('/api/mcp/servers', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        name: slugify(l.title),
        label: l.title,
        url: l.url,
        description: l.description,
        headers: opts.headers,
        authMode: opts.authMode,
        requiredHeaders: l.requiredHeaders,
      }),
    })
    setBusyAdd(null)
    if (!r.ok) {
      setError(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed')
      return false
    }
    const { server } = (await r.json()) as { server: { id: string } }
    setAdded((prev) => new Set(prev).add(l.registryName))
    setInstalling(null)
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    void patchServer(server.id, { refreshTools: true }).then(() => qc.invalidateQueries({ queryKey: ['mcp-servers'] }))
    return true
  }

  // Servers declaring credentials get the install dialog; the rest one-click.
  const install = (l: LibraryServerRow) => {
    if (l.requiredHeaders.length > 0) setInstalling(l)
    else void register(l, {})
  }

  return (
    <Modal open onClose={onClose} title="MCP marketplace" takeover>
      <div className="flex h-full min-h-0 flex-col gap-4">
        <div className="flex items-center gap-3">
          <Input
            autoFocus
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search the official MCP registry — GitHub, Linear, Notion, Stripe, Vercel…"
            className="max-w-xl"
          />
          {isFetching && <RefreshCw size={14} className="animate-spin text-muted" />}
          <span className="flex-1" />
          <button type="button" onClick={onCustom} className="text-xs text-accent hover:underline">
            Custom / self-hosted server →
          </button>
        </div>
        {!q.trim() && (
          <div className="flex items-center gap-1.5 text-xs text-muted">
            <span className="font-semibold uppercase tracking-wide">Featured</span>
            <InfoTip text="Companies publishing their own MCP server on their own verified domain — the official integrations. Search reaches the whole registry, community servers included." />
          </div>
        )}
        {error && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {error}
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto">
          {!results && (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {Array.from({ length: 9 }, (_, i) => (
                <div key={i} className="rounded-2xl border border-line-subtle p-4">
                  <Skeleton className="h-3.5 w-32 rounded-full" delay={i * 0.08} />
                  <Skeleton className="mt-2 h-2.5 w-24 rounded-full" delay={i * 0.08 + 0.06} />
                  <Skeleton className="mt-3 h-2.5 w-full rounded-full" delay={i * 0.08 + 0.12} />
                  <Skeleton className="mt-1.5 h-2.5 w-4/5 rounded-full" delay={i * 0.08 + 0.18} />
                </div>
              ))}
            </div>
          )}
          {results && results.servers.length === 0 && (
            <EmptyState
              icon="⌁"
              title="No hosted servers match"
              hint="Only servers with a hosted endpoint appear — packages that need a local process can't be one-click added. Try the custom form for self-hosted servers."
            />
          )}
          {results && results.servers.length > 0 && (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {results.servers.map((l) => {
                const installed = installedUrls.has(l.url) || added.has(l.registryName)
                const busy = busyAdd === l.registryName
                const badge = TIER_BADGE[l.tier]
                return (
                  <div
                    key={l.registryName}
                    className="group relative flex flex-col rounded-2xl border border-line-subtle bg-card/30 p-4 transition-all hover:border-line hover:bg-card/60"
                  >
                    <div className="flex items-center gap-3">
                      <ServerMark title={l.title} domain={l.domain} icon={l.icon} size={34} />
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-sm font-semibold text-fg">{l.title}</div>
                        <div className="flex items-center gap-1.5">
                          {l.domain && <span className="truncate text-[11px] text-muted">{l.domain}</span>}
                          <span title={badge.hint} className={cn('shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-medium', badge.cls)}>
                            {badge.label}
                          </span>
                        </div>
                      </div>
                    </div>
                    <p className="mt-2.5 line-clamp-3 min-h-12 font-sans text-xs leading-relaxed text-muted">
                      {l.description ?? 'No description.'}
                    </p>
                    {/* The install control: quiet until the card is engaged. */}
                    <div className="absolute right-3 top-3">
                      {installed ? (
                        <span
                          className="grid h-7 w-7 place-items-center rounded-full bg-[color:var(--theme-success)]/10 text-[color:var(--theme-success)]"
                          title="In your org registry"
                        >
                          <Check size={14} />
                        </span>
                      ) : (
                        <button
                          type="button"
                          title={l.requiredHeaders.length ? 'Add to your org (needs credentials)' : 'Add to your org'}
                          disabled={busy}
                          onClick={() => install(l)}
                          className={cn(
                            'grid h-7 w-7 place-items-center rounded-full border transition-all',
                            busy
                              ? 'border-line bg-card text-muted opacity-100'
                              : 'border-line bg-card text-muted opacity-0 hover:border-accent hover:text-accent group-hover:opacity-100',
                          )}
                        >
                          {busy ? <RefreshCw size={13} className="animate-spin" /> : <Plus size={14} />}
                        </button>
                      )}
                    </div>
                  </div>
                )
              })}
            </div>
          )}
        </div>
        {installing && (
          <InstallDialog
            server={installing}
            busy={busyAdd === installing.registryName}
            onCancel={() => setInstalling(null)}
            onInstall={(opts) => void register(installing, opts)}
          />
        )}
      </div>
    </Modal>
  )
}

/** Credential capture for servers that declare required headers — schema-
 *  driven: secret fields mask, choices become selects, placeholders and
 *  descriptions come from the publisher. Per-user mode skips values here and
 *  lets each person connect their own account in Settings. */
function InstallDialog({
  server: l,
  busy,
  onCancel,
  onInstall,
}: {
  server: LibraryServerRow
  busy: boolean
  onCancel: () => void
  onInstall: (opts: { headers?: Record<string, string>; authMode?: 'org' | 'per-user' }) => void
}) {
  const [authMode, setAuthMode] = useState<'org' | 'per-user'>('org')
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(l.requiredHeaders.filter((h) => h.default).map((h) => [h.name, h.default!])),
  )
  const missing = authMode === 'org' && l.requiredHeaders.some((h) => h.isRequired && !values[h.name]?.trim())
  const submit = () => {
    if (busy || missing) return
    const headers = Object.fromEntries(Object.entries(values).filter(([, v]) => v.trim()))
    onInstall(authMode === 'org' ? { headers } : { authMode: 'per-user' })
  }

  return (
    <div className="fixed inset-0 z-[60] grid place-items-center bg-black/40 p-4" onClick={onCancel}>
      <div className="w-full max-w-md rounded-2xl border border-line bg-surface p-5 shadow-xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-3">
          <ServerMark title={l.title} domain={l.domain} icon={l.icon} size={36} />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-semibold text-fg">Connect {l.title}</div>
            {l.domain && <div className="text-[11px] text-muted">{l.domain}</div>}
          </div>
        </div>
        <p className="mt-2 font-sans text-xs text-muted">This server declares credentials. Choose how your org authenticates:</p>
        <div className="mt-3">
          <Combobox
            options={[
              { value: 'org', label: 'Org account', sub: 'one shared credential, stored on the server row' },
              { value: 'per-user', label: 'Per-user accounts', sub: 'each person connects their own in Settings' },
            ]}
            selected={[authMode]}
            onChange={([v]) => v && setAuthMode(v as 'org' | 'per-user')}
            placeholder="Auth mode"
          />
        </div>
        {authMode === 'org' && (
          <div className="mt-3 space-y-3">
            {l.requiredHeaders.map((h) => (
              <div key={h.name}>
                <label className="mb-1 flex items-baseline gap-1.5 text-[11px] uppercase tracking-wide text-muted">
                  {h.name}
                  {h.isRequired && <span className="text-accent">*</span>}
                </label>
                {h.choices?.length ? (
                  <Select size="sm" value={values[h.name] ?? ''} onChange={(e) => setValues((p) => ({ ...p, [h.name]: e.target.value }))} className="w-full">
                    <option value="">Pick one</option>
                    {h.choices.map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input
                    type={h.isSecret ? 'password' : 'text'}
                    value={values[h.name] ?? ''}
                    onChange={(e) => setValues((p) => ({ ...p, [h.name]: e.target.value }))}
                    placeholder={h.placeholder ?? (h.isSecret ? '•••' : '')}
                    autoComplete="off"
                  />
                )}
                {h.description && <div className="mt-1 font-sans text-[11px] text-muted/90">{h.description}</div>}
              </div>
            ))}
            <div className="font-sans text-[11px] text-muted/80">Spoken only by Talaria's gateway — never rendered into an agent config.</div>
          </div>
        )}
        {authMode === 'per-user' && (
          <p className="mt-3 font-sans text-xs text-muted">
            Nobody gets access until they connect their own account in Settings → Connections. Their assistant then acts as them on this server.
          </p>
        )}
        <div className="mt-4 flex justify-end gap-2 border-t border-line-subtle pt-3">
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button size="sm" disabled={busy || missing} onClick={submit}>
            {busy ? 'Connecting' : 'Add to org'}
          </Button>
        </div>
      </div>
    </div>
  )
}

/** Credentials form for providers without dynamic client registration: shows
 *  the exact callback URL to register, takes the app's client id/secret. */
// Where to create the app, for providers we recognize. One-time, org-owner.
const OAUTH_APP_PORTALS: Record<string, string> = {
  'api.githubcopilot.com': 'https://github.com/settings/developers',
}

function OauthAppSetup({ serverId, domain, onSaved }: { serverId: string; domain?: string | null; onSaved: () => void }) {
  const [openForm, setOpenForm] = useState(false)
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const callback = `${window.location.origin}/api/mcp/oauth/callback`

  const save = async () => {
    setBusy(true)
    setError(null)
    const e = await patchServer(serverId, { oauthClient: { clientId: clientId.trim(), clientSecret: clientSecret.trim() || null } })
    setBusy(false)
    if (e) {
      setError(e)
      return
    }
    setOpenForm(false)
    onSaved()
  }

  return (
    <div className="mt-3 rounded-xl border border-accent/40 bg-accent/5 p-3">
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-fg">This provider needs a pre-registered OAuth app</span>
        <InfoTip text="It doesn't support automatic client registration (GitHub, for example). Create an OAuth app in the provider's developer settings with the callback URL below, then paste the app's client id and secret here — stored encrypted, spoken only during the OAuth flow." />
        <span className="flex-1" />
        {domain && OAUTH_APP_PORTALS[domain] && (
          <a href={OAUTH_APP_PORTALS[domain]} target="_blank" rel="noreferrer" className="text-xs text-accent hover:underline">
            Create the app ↗
          </a>
        )}
        {!openForm && (
          <Button size="sm" onClick={() => setOpenForm(true)}>
            Set up
          </Button>
        )}
      </div>
      {openForm && (
        <div className="mt-3 space-y-3">
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Callback URL (register this with the provider)</label>
            <div className="flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded-lg bg-card px-2 py-1.5 text-xs text-fg">{callback}</code>
              <Button size="sm" variant="ghost" onClick={() => void navigator.clipboard.writeText(callback)}>
                Copy
              </Button>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Client ID</label>
              <Input value={clientId} onChange={(e) => setClientId(e.target.value)} autoComplete="off" />
            </div>
            <div>
              <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Client secret</label>
              <Input type="password" value={clientSecret} onChange={(e) => setClientSecret(e.target.value)} autoComplete="off" />
            </div>
          </div>
          {error && (
            <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" onClick={() => setOpenForm(false)}>
              Cancel
            </Button>
            <Button size="sm" disabled={busy || !clientId.trim()} onClick={() => void save()}>
              {busy ? 'Saving' : 'Save credentials'}
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}

function AddServerModal({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const [label, setLabel] = useState('')
  const [slugEdited, setSlugEdited] = useState(false)
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [description, setDescription] = useState('')
  const [headerKey, setHeaderKey] = useState('')
  const [headerVal, setHeaderVal] = useState('')
  const [authMode, setAuthMode] = useState<'org' | 'per-user'>('org')
  const [timeoutSecs, setTimeoutSecs] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const slugify = (v: string) =>
    v.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)
  const setLabelAndSlug = (v: string) => {
    setLabel(v)
    if (!slugEdited) setName(slugify(v))
  }
  const valid = !!name.trim() && /^https?:\/\//.test(url.trim())

  const submit = async () => {
    if (!valid || busy) return
    setBusy(true)
    setError(null)
    const r = await fetch('/api/mcp/servers', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        name: name.trim(),
        label: label.trim() || undefined,
        url: url.trim(),
        description: description.trim() || null,
        headers: headerKey.trim() && headerVal.trim() ? { [headerKey.trim()]: headerVal } : undefined,
        timeoutSecs: timeoutSecs.trim() ? Number(timeoutSecs) : undefined,
        authMode,
      }),
    })
    setBusy(false)
    if (!r.ok) {
      setError(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed')
      return
    }
    const { server } = (await r.json()) as { server: { id: string } }
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    // Kick discovery right away — a server with no tool catalog is half-registered.
    void patchServer(server.id, { refreshTools: true }).then(() => qc.invalidateQueries({ queryKey: ['mcp-servers'] }))
    onClose()
  }
  const onEnter = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      void submit()
    }
  }

  return (
    <Modal open onClose={onClose} title="Custom MCP server">
      <div className="space-y-4">
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
          <Input autoFocus value={label} onChange={(e) => setLabelAndSlug(e.target.value)} onKeyDown={onEnter} placeholder="GitHub" />
          {name && (
            <div className="mt-1 flex items-center gap-1.5 text-[11px] text-muted">
              agents will know it as
              <input
                value={name}
                onChange={(e) => {
                  setSlugEdited(true)
                  setName(slugify(e.target.value))
                }}
                className="rounded bg-card px-1.5 py-0.5 font-medium text-fg outline-none"
                size={Math.max(name.length, 4)}
              />
            </div>
          )}
        </div>
        <div>
          <label className="mb-1 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted">
            Server URL
            <InfoTip text="The MCP endpoint (streamable HTTP). Agents never see this address — they go through Talaria's gateway, which enforces the access rules you set here." />
          </label>
          <Input value={url} onChange={(e) => setUrl(e.target.value)} onKeyDown={onEnter} placeholder="https://mcp.example.com/mcp" />
        </div>
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Description</label>
          <Input value={description} onChange={(e) => setDescription(e.target.value)} onKeyDown={onEnter} placeholder="What agents get from it (shown in pickers)" />
        </div>
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Authentication</label>
          <Combobox
            options={[
              { value: 'org', label: 'Org account', sub: 'one shared credential for every agent' },
              { value: 'per-user', label: 'Per-user accounts', sub: 'each person connects their own (Settings → Connections)' },
            ]}
            selected={[authMode]}
            onChange={([v]) => v && setAuthMode(v as 'org' | 'per-user')}
            placeholder="Auth mode"
          />
        </div>
        {authMode === 'org' && (
          <div className="grid grid-cols-[10rem_1fr] gap-2">
            <div>
              <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Auth header</label>
              <Input value={headerKey} onChange={(e) => setHeaderKey(e.target.value)} onKeyDown={onEnter} placeholder="Authorization" />
            </div>
            <div>
              <label className="mb-1 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted">
                Value
                <InfoTip text="Stored on the server row and spoken only by the gateway — never rendered into an agent config, never echoed back to this UI." />
              </label>
              <Input type="password" value={headerVal} onChange={(e) => setHeaderVal(e.target.value)} onKeyDown={onEnter} placeholder="Bearer …" autoComplete="off" />
            </div>
          </div>
        )}
        <details>
          <summary className="cursor-pointer text-[11px] uppercase tracking-wide text-muted">Advanced</summary>
          <div className="mt-2">
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Timeout (seconds)</label>
            <Input value={timeoutSecs} onChange={(e) => setTimeoutSecs(e.target.value.replace(/[^0-9]/g, ''))} placeholder="120" className="w-32" />
          </div>
        </details>
        {error && (
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {error}
          </div>
        )}
        <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={busy || !valid}>
            {busy ? 'Registering' : 'Register server'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

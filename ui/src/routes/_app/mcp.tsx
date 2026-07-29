import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, RefreshCw, Trash2 } from 'lucide-react'
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
  tools: Array<{ name: string; description?: string }>
  toolsRefreshedAt: string | null
  assignments: Array<{ agentModel: string; tools: string[] | null }>
  userAccess: Array<{ userId: string; allowed: boolean; tools: string[] | null }>
}

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
  const { data: servers, isPending } = useMcpServers()
  const [adding, setAdding] = useState(false)

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-6">
        <div className="flex items-center gap-2">
          <h1 className="mercury-text text-2xl font-semibold">MCP</h1>
          <InfoTip text="Model Context Protocol servers, managed org-wide. Register once; choose which agents carry each server and which people may use it — down to individual tools. Agents reach servers only through Talaria's gateway, so the limits here are enforced, not advisory." />
          <span className="flex-1" />
          <Button size="sm" onClick={() => setAdding(true)}>
            <Plus size={14} /> Add server
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
            hint="Register your first server — any MCP endpoint your agents should be able to reach."
            action={<Button size="sm" onClick={() => setAdding(true)}>Add server</Button>}
          />
        ) : (
          (servers ?? []).map((s) => <ServerCard key={s.id} server={s} />)
        )}

        {adding && <AddServerModal onClose={() => setAdding(false)} />}
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
  const [libQ, setLibQ] = useState('')
  const [libOpen, setLibOpen] = useState(true)
  const { data: library = [], isFetching: libLoading } = useQuery({
    queryKey: ['mcp-library', libQ],
    queryFn: async (): Promise<Array<{ registryName: string; title: string; description: string | null; url: string }>> => {
      const r = await fetch(`/api/mcp/library?q=${encodeURIComponent(libQ)}`, { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { servers: Array<{ registryName: string; title: string; description: string | null; url: string }> }).servers
    },
    placeholderData: (prev) => prev,
  })

  const pickFromLibrary = (l: { title: string; description: string | null; url: string }) => {
    setLabel(l.title)
    if (!slugEdited) setName(l.title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40))
    setUrl(l.url)
    setDescription(l.description ?? '')
    setLibOpen(false)
  }

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
    <Modal open onClose={onClose} title="Add MCP server">
      <div className="space-y-4">
        {/* The library: the official MCP registry, live. Pick to prefill —
            everything below stays editable, custom servers just type over it. */}
        <div>
          <label className="mb-1 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted">
            Library
            <InfoTip text="The official MCP registry, searched live — servers with a hosted (streamable-HTTP) endpoint. Picking one prefills the form; anything can be edited, and fully custom servers just fill the fields below." />
          </label>
          <Input
            value={libQ}
            onChange={(e) => {
              setLibQ(e.target.value)
              setLibOpen(true)
            }}
            placeholder="Search common services — GitHub, Linear, Notion, Stripe…"
          />
          {libOpen && (libLoading || library.length > 0) && (
            <div className="mt-1 max-h-44 overflow-y-auto rounded-xl border border-line-subtle bg-card/50 p-1">
              {libLoading && library.length === 0 && <SkeletonRows rows={3} className="px-2 py-1.5" />}
              {library.map((l) => (
                <button
                  key={l.registryName}
                  type="button"
                  onClick={() => pickFromLibrary(l)}
                  className="flex w-full items-baseline gap-2 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-sidebar"
                >
                  <span className="shrink-0 text-sm text-fg">{l.title}</span>
                  <span className="min-w-0 flex-1 truncate font-sans text-xs text-muted">{l.description}</span>
                </button>
              ))}
            </div>
          )}
        </div>
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

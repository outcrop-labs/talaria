import { createFileRoute, Link } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Download, ExternalLink, RefreshCw, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Chip } from '@/components/ui/chip'
import { EmptyState } from '@/components/ui/empty-state'
import { SkeletonRows } from '@/components/ui/skeleton'
import { confirm, alert } from '@/components/ui/confirm'
import { InfoTip } from '@/components/ui/info-tip'
import { QueryError } from '@/components/ui/query-state'
import { Tabs } from '@/components/ui/tabs'
import { getJson } from '@/lib/fetch-json'
import { useSession } from '@/lib/session'
import { cn } from '@/lib/cn'

// Manage → Apps: the platform's app system in one place. Installed = what
// this deployment ships (enable/disable, uninstall); Discover = the
// marketplace — community + official apps from the catalog feed, plus
// install-from-git for anything else. Installing clones the app's codebase
// into the deployment: dev picks it up live, prod needs a rebuild.
export const Route = createFileRoute('/_app/apps')({
  component: AppsPage,
})

interface InstalledApp {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  surfaces: { work?: string; manage?: string; settings?: string }
  mcp?: boolean
  enabled: boolean
  source: string | null
}
interface CatalogApp {
  slug: string
  name: string
  icon: string
  description: string
  repo: string
  author: string
  official: boolean
  version?: string
}
interface AdminApps {
  apps: InstalledApp[]
  pending: string[]
  catalog: { apps: CatalogApp[]; error?: string } | null
  catalogUrl: string
}

// `if (!r.ok) return null` made a 500 resolve as a SUCCESSFUL query carrying
// null, so React Query never entered its error state and both consumers below
// fell through to their empty states: a broken read rendered "No apps
// installed", pixel-for-pixel identical to the honest 200-with-nothing. Non-2xx
// throws; empty and broken are different answers.
const fetchAdminApps = (withCatalog: boolean): Promise<AdminApps> =>
  getJson<AdminApps>(`/api/admin/apps${withCatalog ? '?catalog=1' : ''}`)

const post = async (body: Record<string, unknown>): Promise<{ error?: string; slug?: string; pendingBuild?: boolean }> => {
  // POST installs (the action); PUT writes config (enable/disable, catalog).
  const r = await fetch('/api/admin/apps', {
    method: 'installUrl' in body ? 'POST' : 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  return r.json()
}

function AppsPage() {
  const { data: user } = useSession()
  const isAdmin = user?.role === 'admin'
  const [tab, setTab] = useState<'installed' | 'discover'>('installed')

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto w-full max-w-4xl">
        <div className="mb-1 flex items-baseline gap-3">
          <h1 className="font-sans text-2xl font-semibold tracking-tight text-fg">Apps</h1>
          <InfoTip text="Self-contained apps that compile into this deployment and render as native Talaria surfaces — built by your team, the community, or Outcrop. They run under each signed-in user's session, so platform permissions apply unchanged." />
        </div>
        <p className="mb-6 font-sans text-xs text-muted">
          Extend Talaria with new work and manage views. Apps live in the <code className="text-fg">apps/</code> directory and become part of the deployment.
        </p>

        <Tabs
          className="mb-6"
          items={[
            { id: 'installed', label: 'Installed' },
            { id: 'discover', label: 'Discover' },
          ]}
          value={tab}
          onChange={setTab}
        />

        {tab === 'installed' ? <InstalledTab isAdmin={isAdmin} /> : <DiscoverTab isAdmin={isAdmin} />}
      </div>
    </div>
  )
}

function SurfaceChips({ surfaces, mcp }: { surfaces: InstalledApp['surfaces']; mcp?: boolean }) {
  return (
    <span className="flex gap-1">
      {surfaces.work && <Chip title={`Work view: ${surfaces.work}`}>work</Chip>}
      {surfaces.manage && <Chip title={`Manage view: ${surfaces.manage}`}>manage</Chip>}
      {surfaces.settings && <Chip title={`Settings panel: ${surfaces.settings}`}>settings</Chip>}
      {mcp && <Chip title="Publishes MCP tools for agents — govern access in Manage → MCP">mcp</Chip>}
    </span>
  )
}

function InstalledTab({ isAdmin }: { isAdmin: boolean }) {
  const qc = useQueryClient()
  const query = useQuery({ queryKey: ['admin-apps'], queryFn: () => fetchAdminApps(false) })
  const { data, isLoading } = query
  const [busy, setBusy] = useState<string | null>(null)

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['admin-apps'] })
    await qc.invalidateQueries({ queryKey: ['apps'] })
    await qc.invalidateQueries({ queryKey: ['session'] })
  }

  const toggle = async (a: InstalledApp) => {
    setBusy(a.slug)
    try {
      const r = await post({ app: a.slug, enabled: !a.enabled })
      if (r.error) void alert({ title: 'Could not update app', message: r.error })
      await refresh()
    } finally {
      setBusy(null)
    }
  }

  const uninstall = async (a: InstalledApp) => {
    const ok = await confirm({
      title: `Uninstall ${a.name}?`,
      message: 'Removes the app codebase from this deployment and deletes the data it stored. This cannot be undone.',
      confirmLabel: 'Uninstall',
      danger: true,
    })
    if (!ok) return
    setBusy(a.slug)
    try {
      const r = await fetch('/api/admin/apps', {
        method: 'DELETE',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ app: a.slug, wipeData: true }),
      })
      const j = (await r.json()) as { error?: string }
      if (j.error) void alert({ title: 'Could not uninstall', message: j.error })
      await refresh()
    } finally {
      setBusy(null)
    }
  }

  if (isLoading) return <SkeletonRows rows={4} />
  // Without this branch a failed read told an admin their deployment has no
  // apps — the same sentence the honest empty case shows, and the one that
  // sends someone reinstalling an app that was never gone.
  if (!data) return <QueryError error={query.error} title="Could not load installed apps" onRetry={() => void query.refetch()} />
  const apps = data.apps ?? []
  const pending = data.pending ?? []

  if (apps.length === 0 && pending.length === 0) {
    return (
      <EmptyState
        icon="⬡"
        title="No apps installed"
        hint="Discover community and official apps in the next tab, or drop a codebase into apps/ — see apps/README.md for building your own"
      />
    )
  }

  return (
    <div className="space-y-3">
      {apps.map((a) => (
        <div key={a.slug} className="flex items-center gap-4 rounded-lg border border-line bg-panel p-4">
          <span className="w-8 text-center text-2xl text-accent">{a.icon}</span>
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline gap-2">
              <span className="font-sans text-sm font-medium text-fg">{a.name}</span>
              <span className="font-mono text-[10px] tracking-[0.05em] text-muted">v{a.version}</span>
              <SurfaceChips surfaces={a.surfaces} mcp={a.mcp} />
            </div>
            <div className="truncate font-sans text-xs text-muted">{a.description}</div>
          </div>
          {a.enabled && a.surfaces.work && (
            <Link to="/x/$app" params={{ app: a.slug }} className="flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
              <ExternalLink size={12} /> Open
            </Link>
          )}
          {isAdmin && (
            <>
              <Button size="sm" variant={a.enabled ? 'ghost' : 'primary'} disabled={busy === a.slug} onClick={() => void toggle(a)}>
                {a.enabled ? 'Disable' : 'Enable'}
              </Button>
              <button
                title="Uninstall"
                disabled={busy === a.slug}
                onClick={() => void uninstall(a)}
                className="text-muted transition-colors hover:text-danger"
              >
                <Trash2 size={14} />
              </button>
            </>
          )}
        </div>
      ))}
      {pending.map((slug) => (
        <div key={slug} className="flex items-center gap-4 rounded-lg border border-dashed border-line bg-panel p-4 opacity-80">
          <span className="w-8 text-center text-2xl text-accent">⧗</span>
          <div className="min-w-0 flex-1">
            <div className="font-sans text-sm font-medium text-fg">{slug}</div>
            <div className="font-sans text-xs text-muted">
              Installed on disk but not compiled into this build yet — reload the dev server or rebuild the deployment to activate.
            </div>
          </div>
          <Chip>awaiting build</Chip>
        </div>
      ))}
    </div>
  )
}

function DiscoverTab({ isAdmin }: { isAdmin: boolean }) {
  const qc = useQueryClient()
  const query = useQuery({
    queryKey: ['admin-apps-catalog'],
    queryFn: () => fetchAdminApps(true),
    staleTime: 5 * 60_000,
  })
  const { data, isLoading, refetch, isFetching } = query
  const [gitUrl, setGitUrl] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const installed = new Set((data?.apps ?? []).map((a) => a.slug).concat(data?.pending ?? []))

  const install = async (url: string, slug?: string) => {
    setBusy(slug ?? url)
    setNotice(null)
    try {
      const r = await post({ installUrl: url, ...(slug ? { slug } : {}) })
      if (r.error) {
        void alert({ title: 'Install failed', message: r.error })
      } else {
        setNotice(
          r.pendingBuild
            ? `Installed apps/${r.slug}. It compiles into the next build — reload the dev server (or rebuild) then enable it under Installed.`
            : `Installed apps/${r.slug} — enable it under Installed.`,
        )
        setGitUrl('')
        await qc.invalidateQueries({ queryKey: ['admin-apps'] })
        await qc.invalidateQueries({ queryKey: ['admin-apps-catalog'] })
      }
    } finally {
      setBusy(null)
    }
  }

  const catalog = data?.catalog
  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div className="font-sans text-xs text-muted">
          Community and official apps from the marketplace index.
          <InfoTip text="Installing an app clones its repository into this deployment and its code runs fully trusted, like the platform itself. Install only apps you trust — official apps are maintained by Outcrop Labs." />
        </div>
        <button
          onClick={() => void refetch()}
          className={cn('flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg', isFetching && 'gd-breathe')}
        >
          <RefreshCw size={12} /> Refresh
        </button>
      </div>

      {notice && <div className="rounded-lg border border-line bg-card px-4 py-3 font-sans text-xs text-fg">{notice}</div>}

      {isLoading ? (
        <SkeletonRows rows={4} />
      ) : !data ? (
        // Distinct from "Marketplace unreachable" below: THAT is the server
        // telling us it could not reach the catalog feed (a real, reported
        // answer, with `catalog.error` to quote). This is OUR read of the
        // server failing, which says nothing about the marketplace at all.
        <QueryError error={query.error} title="Could not load apps" onRetry={() => void refetch()} />
      ) : (catalog?.apps ?? []).length === 0 ? (
        <EmptyState
          icon="◎"
          title="Marketplace unreachable"
          hint={catalog?.error ? `${catalog.error} — you can still install any app from its git URL below` : 'No apps in the catalog yet — install from a git URL below'}
        />
      ) : (
        <div className="grid gap-3 sm:grid-cols-2">
          {catalog!.apps.map((c) => (
            <div key={c.slug} className="flex flex-col gap-2 rounded-lg border border-line bg-panel p-4">
              <div className="flex items-center gap-3">
                <span className="text-2xl text-accent">{c.icon}</span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-baseline gap-2">
                    <span className="truncate font-sans text-sm font-medium text-fg">{c.name}</span>
                    {c.official && <Chip tone="accent">official</Chip>}
                  </div>
                  <div className="font-mono text-[10px] tracking-[0.05em] text-muted">by {c.author}</div>
                </div>
              </div>
              <div className="flex-1 font-sans text-xs text-muted">{c.description}</div>
              <div className="flex items-center justify-between">
                <a href={c.repo} target="_blank" rel="noreferrer" className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted underline-offset-2 hover:underline">
                  source
                </a>
                {installed.has(c.slug) ? (
                  <Chip>installed</Chip>
                ) : isAdmin ? (
                  <Button size="sm" disabled={busy !== null} onClick={() => void install(c.repo, c.slug)}>
                    <Download size={12} /> Install
                  </Button>
                ) : null}
              </div>
            </div>
          ))}
        </div>
      )}

      {isAdmin && (
        <div className="rounded-lg border border-line bg-panel p-4">
          <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Install from Git</div>
          <p className="mb-3 font-sans text-xs text-muted">
            Any https git repository with a <code className="text-fg">talaria.json</code> at its root. The code becomes part of this
            deployment and runs fully trusted — install only repositories you trust.
          </p>
          <div className="flex gap-2">
            <Input
              value={gitUrl}
              onChange={(e) => setGitUrl(e.target.value)}
              placeholder="https://github.com/you/talaria-app-yourthing"
              className="flex-1"
            />
            <Button disabled={!/^https:\/\/.+/.test(gitUrl.trim()) || busy !== null} onClick={() => void install(gitUrl.trim())}>
              Install
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}

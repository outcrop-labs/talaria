<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ExternalLink, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm, alert } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { p } from '@/router'
  import { delJson, errorMessage, postJson } from '@/lib/fetch-json'
  import { fetchAdminApps, post, type InstalledApp } from './apps'

  let { isAdmin }: { isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({ queryKey: ['admin-apps'], queryFn: () => fetchAdminApps(false) }))
  let busy = $state<string | null>(null)

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['admin-apps'] })
    await qc.invalidateQueries({ queryKey: ['apps'] })
    await qc.invalidateQueries({ queryKey: ['session'] })
  }

  const toggle = async (a: InstalledApp) => {
    busy = a.slug
    try {
      if (!a.enabled) {
        const health = await postJson<{ ok: boolean; error?: string }>(`/api/apps/${a.slug}/rebuild`, {})
        if (!health.ok) {
          void alert({ title: `${a.name} cannot be enabled`, message: health.error ?? 'app failed to compile or load' })
          await refresh()
          return
        }
      }
      const r = await post({ app: a.slug, enabled: !a.enabled })
      if (r.error) void alert({ title: 'Could not update app', message: r.error })
      await refresh()
    } catch (e) {
      void alert({ title: 'Could not update app', message: errorMessage(e) })
    } finally {
      busy = null
    }
  }

  const rebuild = async (a: InstalledApp) => {
    busy = a.slug
    try {
      const health = await postJson<{ ok: boolean; error?: string }>(`/api/apps/${a.slug}/rebuild`, {})
      if (!health.ok) void alert({ title: `${a.name} is still broken`, message: health.error ?? 'app failed to compile or load' })
      await refresh()
    } catch (e) {
      void alert({ title: 'Could not rebuild', message: errorMessage(e) })
    } finally {
      busy = null
    }
  }

  const uninstall = async (a: InstalledApp) => {
    const ok = await confirm({
      title: `Uninstall ${a.name}?`,
      message: 'Removes the app codebase from this deployment, stops its database, and deletes the data it stored. This cannot be undone.',
      confirmLabel: 'Uninstall',
      danger: true,
    })
    if (!ok) return
    busy = a.slug
    try {
      await delJson<{ ok: true }>('/api/admin/apps', { app: a.slug, wipeData: true }).catch((e) => {
        void alert({ title: 'Could not uninstall', message: errorMessage(e) })
      })
      await refresh()
    } finally {
      busy = null
    }
  }

  const apps = $derived(query.data?.apps ?? [])
</script>

{#snippet surfaceChips(surfaces: InstalledApp['surfaces'], mcp?: boolean)}
  <span class="flex gap-1">
    {#if surfaces.work}<Chip title={`Work view: ${surfaces.work}`}>work</Chip>{/if}
    {#if surfaces.manage}<Chip title={`Manage view: ${surfaces.manage}`}>manage</Chip>{/if}
    {#if surfaces.settings}<Chip title={`Settings panel: ${surfaces.settings}`}>settings</Chip>{/if}
    {#if mcp}<Chip title="Publishes MCP tools for agents. Govern access in Manage → MCP">mcp</Chip>{/if}
  </span>
{/snippet}

{#if query.isLoading}
  <SkeletonRows rows={4} />
{:else if !query.data}
  <QueryError error={query.error} title="Could not load installed apps" onRetry={() => void query.refetch()} />
{:else if apps.length === 0}
  <EmptyState
    icon="⬡"
    title="No apps installed"
    hint="Discover community and official apps in the next tab, or drop a codebase into apps/ (see apps/README.md for building your own)"
  />
{:else}
  <div class="space-y-3" data-app-cards>
    {#each apps as a (a.slug)}
      <div class="flex items-center gap-4 rounded-lg border border-line bg-panel p-4">
        <span class="w-8 text-center text-2xl text-accent">{a.icon}</span>
        <div class="min-w-0 flex-1">
          <div class="flex items-baseline gap-2">
            <span class="font-sans text-sm font-medium text-fg">{a.name}</span>
            <span class="font-mono text-[10px] tracking-[0.05em] text-muted">v{a.version}</span>
            {@render surfaceChips(a.surfaces, a.mcp)}
            {#if a.build?.status === 'building'}<Chip>compiling</Chip>{/if}
            {#if a.build?.status === 'failed'}<Chip tone="danger" title={a.build.error}>build failed</Chip>{/if}
          </div>
          <div class="truncate font-sans text-xs text-muted">{a.description}</div>
          {#if a.build?.status === 'failed' && a.build.error}
            <div class="mt-1 truncate font-mono text-[11px] text-danger">{a.build.error}</div>
          {/if}
        </div>
        {#if a.enabled && a.surfaces.work}
          <a href={p('/x/:app', { params: { app: a.slug } })} class="flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
            <ExternalLink size={12} /> Open
          </a>
        {/if}
        {#if isAdmin}
          {#if a.build?.status === 'failed'}
            <Button size="sm" variant="ghost" disabled={busy === a.slug} onclick={() => void rebuild(a)}>Rebuild</Button>
          {/if}
          <Button size="sm" variant={a.enabled ? 'ghost' : 'primary'} disabled={busy === a.slug} onclick={() => void toggle(a)}>
            {a.enabled ? 'Disable' : 'Enable'}
          </Button>
          <button
            title="Uninstall"
            disabled={busy === a.slug}
            onclick={() => void uninstall(a)}
            class="text-muted transition-colors hover:text-danger"
          >
            <Trash2 size={14} />
          </button>
        {/if}
      </div>
    {/each}
  </div>
{/if}

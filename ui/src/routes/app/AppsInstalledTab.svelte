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
      const r = await post({ app: a.slug, enabled: !a.enabled })
      if (r.error) void alert({ title: 'Could not update app', message: r.error })
      await refresh()
    } finally {
      busy = null
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
    busy = a.slug
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
      busy = null
    }
  }

  const apps = $derived(query.data?.apps ?? [])
  const pending = $derived(query.data?.pending ?? [])
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
  <!-- Without this branch a failed read told an admin their deployment has no
       apps — the same sentence the honest empty case shows, and the one that
       sends someone reinstalling an app that was never gone. -->
  <QueryError error={query.error} title="Could not load installed apps" onRetry={() => void query.refetch()} />
{:else if apps.length === 0 && pending.length === 0}
  <EmptyState
    icon="⬡"
    title="No apps installed"
    hint="Discover community and official apps in the next tab, or drop a codebase into apps/ (see apps/README.md for building your own)"
  />
{:else}
  <!-- data-app-cards: the hook Apps.svelte's data-stagger-items selector
       targets — these cards cascade on the page entrance. -->
  <div class="space-y-3" data-app-cards>
    {#each apps as a (a.slug)}
      <div class="flex items-center gap-4 rounded-lg border border-line bg-panel p-4">
        <span class="w-8 text-center text-2xl text-accent">{a.icon}</span>
        <div class="min-w-0 flex-1">
          <div class="flex items-baseline gap-2">
            <span class="font-sans text-sm font-medium text-fg">{a.name}</span>
            <span class="font-mono text-[10px] tracking-[0.05em] text-muted">v{a.version}</span>
            {@render surfaceChips(a.surfaces, a.mcp)}
          </div>
          <div class="truncate font-sans text-xs text-muted">{a.description}</div>
        </div>
        {#if a.enabled && a.surfaces.work}
          <a href={p('/x/:app', { params: { app: a.slug } })} class="flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
            <ExternalLink size={12} /> Open
          </a>
        {/if}
        {#if isAdmin}
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
    {#each pending as slug (slug)}
      <div class="flex items-center gap-4 rounded-lg border border-dashed border-line bg-panel p-4 opacity-80">
        <span class="w-8 text-center text-2xl text-accent">⧗</span>
        <div class="min-w-0 flex-1">
          <div class="font-sans text-sm font-medium text-fg">{slug}</div>
          <div class="font-sans text-xs text-muted">
            Installed on disk but not compiled into this build yet. Reload the dev server or rebuild the deployment to activate.
          </div>
        </div>
        <Chip>awaiting build</Chip>
      </div>
    {/each}
  </div>
{/if}

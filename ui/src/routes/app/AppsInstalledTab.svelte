<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ExternalLink, GitBranch, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm, alert } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { p } from '@/router'
  import { delJson, errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { fetchAdminApps, fetchAppUpdates, post, shortSha, type AppUpdateResult, type InstalledApp } from './apps'

  let { isAdmin }: { isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({ queryKey: ['admin-apps'], queryFn: () => fetchAdminApps(false) }))
  // Whether each app's source has a newer commit: `git ls-remote` against its
  // recorded ref, admin only (a check spends the app's read credential).
  const updates = createQuery(() => ({
    queryKey: ['admin-apps-updates'],
    queryFn: fetchAppUpdates,
    enabled: isAdmin,
    staleTime: 60_000,
  }))
  let busy = $state<string | null>(null)
  /** The last update's outcome, per app, shown on its card. */
  let results = $state<Record<string, { ok: boolean; text: string }>>({})

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['admin-apps'] })
    await qc.invalidateQueries({ queryKey: ['admin-apps-updates'] })
    await qc.invalidateQueries({ queryKey: ['apps'] })
    await qc.invalidateQueries({ queryKey: ['session'] })
  }

  const update = async (a: InstalledApp) => {
    busy = a.slug
    delete results[a.slug]
    try {
      const r = await putJson<AppUpdateResult>('/api/admin/apps', { app: a.slug, update: true })
      results[a.slug] = r.upToDate
        ? { ok: true, text: `Already up to date (${shortSha(r.to)})` }
        : r.ok
          ? { ok: true, text: `Updated ${shortSha(r.from)} → ${shortSha(r.to)}` }
          : { ok: false, text: `Update to ${shortSha(r.to)} failed and was rolled back; still running ${shortSha(r.from)}: ${r.error ?? 'unknown error'}` }
      await refresh()
    } catch (e) {
      results[a.slug] = { ok: false, text: `Could not update: ${errorMessage(e)}` }
    } finally {
      busy = null
    }
  }

  const toggle = async (a: InstalledApp) => {
    busy = a.slug
    try {
      if (!a.enabled) {
        const health = await postJson<{ ok: boolean; error?: string }>(`/api/app-host/${a.slug}/rebuild`, {})
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
      const health = await postJson<{ ok: boolean; error?: string }>(`/api/app-host/${a.slug}/rebuild`, {})
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
      message: 'Removes the app codebase and its build from this deployment and stops its database. Agents lose its MCP server and every grant on it.',
      confirmLabel: 'Uninstall',
      danger: true,
    })
    if (!ok) return
    // Data is kept unless the admin says otherwise: a reinstall reattaches it.
    const wipeData = await confirm({
      title: `Delete the data ${a.name} stored?`,
      message: 'Keep it, and reinstalling the app picks it up again. Delete it, and every record the app stored is gone for good.',
      confirmLabel: 'Delete data',
      cancelLabel: 'Keep data',
      danger: true,
    })
    busy = a.slug
    try {
      await delJson<{ ok: true; data: 'wiped' | 'kept' | 'none' }>('/api/admin/apps', { app: a.slug, wipeData }).catch((e) => {
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

<!-- The activity harnesses the app ships (harnesses/*.ts), as the last load
     probe checked them: the ones registered for model fitness, and the ones
     skipped. -->
{#snippet harnessChips(a: InstalledApp)}
  {@const registered = a.build?.harnesses?.registered ?? []}
  {@const skipped = a.build?.harnesses?.problems ?? []}
  {#if registered.length > 0}
    <Chip title={`Activity harnesses scored in Models → Fitness:\n${registered.join('\n')}`}>
      {registered.length} harness{registered.length === 1 ? '' : 'es'}
    </Chip>
  {/if}
  {#if skipped.length > 0 && a.build?.status !== 'failed'}
    <Chip tone="warn" title={skipped.join('\n')}>{skipped.length} harness{skipped.length === 1 ? '' : 'es'} skipped</Chip>
  {/if}
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
            {#if a.version}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">v{a.version}</span>{/if}
            {@render surfaceChips(a.surfaces, a.mcp)}
            {#if a.broken}<Chip tone="danger" title={a.broken}>broken manifest</Chip>
            {:else if a.build?.status === 'building'}<Chip>compiling</Chip>
            {:else if a.build?.status === 'failed'}<Chip tone="danger" title={a.build.error}>build failed</Chip>{/if}
            {@render harnessChips(a)}
          </div>
          <div class="truncate font-sans text-xs text-muted">{a.description}</div>
          {#if a.git}
            {@const check = updates.data?.[a.slug]}
            <div class="mt-1 flex items-center gap-2 font-mono text-[11px] text-muted" data-app-source>
              <GitBranch size={11} />
              <span class="truncate" title={a.git.url}>{a.git.repo}</span>
              <span class="shrink-0 whitespace-nowrap">@ {a.git.ref ?? 'default branch'}</span>
              <span class="shrink-0" title={a.git.sha ?? 'installed before commits were recorded'}>{shortSha(a.git.sha)}</span>
              {#if check?.available}
                <span class="shrink-0"><Chip tone="accent" title={`The source now points at ${shortSha(check.latest)}`}>update available</Chip></span>
              {:else if check?.error}
                <span class="truncate text-danger" title={check.error}>update check failed</span>
              {/if}
            </div>
          {/if}
          {#if a.broken}
            <div class="mt-1 truncate font-mono text-[11px] text-danger" title={a.broken}>{a.broken}</div>
          {:else if a.build?.status === 'failed' && a.build.error}
            <div class="mt-1 truncate font-mono text-[11px] text-danger">{a.build.error}</div>
          {/if}
          <!-- A harness the platform skipped never disables the app, so this
               line is the only place an admin learns one is missing from the
               fitness matrix, and why. -->
          {#if a.build?.status !== 'failed'}
            {#each a.build?.harnesses?.problems ?? [] as problem (problem)}
              <div class="mt-1 break-words font-mono text-[11px] text-warning" data-harness-problem>{problem}</div>
            {/each}
          {/if}
          {#if results[a.slug]}
            <div class={`mt-1 font-mono text-[11px] ${results[a.slug]!.ok ? 'text-fg' : 'text-danger'}`} data-app-update-result>
              {results[a.slug]!.text}
            </div>
          {/if}
        </div>
        {#if a.enabled && a.surfaces.work}
          <a href={p('/x/:app', { params: { app: a.slug } })} class="flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
            <ExternalLink size={12} /> Open
          </a>
        {/if}
        {#if isAdmin}
          {#if a.git && !a.broken}
            <Button
              size="sm"
              variant={updates.data?.[a.slug]?.available ? 'primary' : 'ghost'}
              disabled={busy === a.slug}
              title="Fetch the newest commit of this app's ref, build and check it, then switch to it; a version that fails is rolled back"
              onclick={() => void update(a)}
            >
              {busy === a.slug ? 'Working…' : 'Update'}
            </Button>
          {/if}
          {#if a.build?.status === 'failed' && !a.broken}
            <Button size="sm" variant="ghost" disabled={busy === a.slug} onclick={() => void rebuild(a)}>Rebuild</Button>
          {/if}
          {#if !a.broken}
            <Button size="sm" variant={a.enabled ? 'ghost' : 'primary'} disabled={busy === a.slug} onclick={() => void toggle(a)}>
              {a.enabled ? 'Disable' : 'Enable'}
            </Button>
          {/if}
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

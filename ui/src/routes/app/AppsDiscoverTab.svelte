<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Download, RefreshCw } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { alert, confirm } from '@/components/ui/confirm.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import { fetchAdminApps, installApp, shortSha, type KeptDataConflict } from './apps'
  import AppsCatalogs from './AppsCatalogs.svelte'

  let { isAdmin }: { isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['admin-apps-catalog'],
    queryFn: () => fetchAdminApps(true),
    staleTime: 5 * 60_000,
  }))
  let gitUrl = $state('')
  let gitRef = $state('')
  let gitSecret = $state('')
  let busy = $state<string | null>(null)
  let notice = $state<string | null>(null)

  const installed = $derived(new Set((query.data?.apps ?? []).map((a) => a.slug)))

  type InstallArgs = { url: string; slug?: string; ref?: string; secret?: string }
  /** An install refused over data another repository's install kept: the
   *  sentence, both repositories, and the request to repeat with a choice. */
  let keptConflict = $state<{ error: string; conflict: KeptDataConflict; args: InstallArgs } | null>(null)

  const install = async (args: InstallArgs, keptData?: 'adopt' | 'wipe') => {
    busy = args.slug ?? args.url
    notice = null
    keptConflict = null
    try {
      const r = await installApp({
        installUrl: args.url,
        ...(args.slug ? { slug: args.slug } : {}),
        ...(args.ref ? { ref: args.ref } : {}),
        ...(args.secret ? { secret: args.secret } : {}),
        ...(keptData ? { keptData } : {}),
      })
      if (r.conflict) {
        keptConflict = { error: r.error ?? 'the slug holds data another install kept', conflict: r.conflict, args }
      } else if (r.error) {
        void alert({ title: 'Install failed', message: r.error })
      } else {
        const built = r.build?.ok
          ? 'It built and loaded; enable it under Installed.'
          : `It did not build: ${r.build?.error ?? r.buildError ?? 'unknown error'}. Fix the app and Rebuild, or uninstall it.`
        const kept = r.keptData
          ? ' The data a previous install kept is attached again.'
          : r.wipedData
            ? ' The data a previous install kept was deleted first.'
            : ''
        notice = `Installed apps/${r.slug} at ${shortSha(r.sha)}. ${built}${kept}`
        gitUrl = ''
        gitRef = ''
        gitSecret = ''
        await qc.invalidateQueries({ queryKey: ['admin-apps'] })
        await qc.invalidateQueries({ queryKey: ['admin-apps-catalog'] })
        await qc.invalidateQueries({ queryKey: ['admin-apps-updates'] })
      }
    } finally {
      busy = null
    }
  }

  /** Repeat the refused install, giving it the kept data or deleting that
   *  data first. Deleting is for good, so it asks once more. */
  const settleKept = async (choice: 'adopt' | 'wipe') => {
    const k = keptConflict
    if (!k) return
    if (choice === 'wipe') {
      const ok = await confirm({
        title: `Delete the data kept under "${k.conflict.slug}"?`,
        message: `Every record ${k.conflict.keptFrom ?? 'the earlier install'} stored is deleted for good, and ${k.conflict.installing} installs with an empty store.`,
        confirmLabel: 'Delete data and install',
        danger: true,
      })
      if (!ok) return
    }
    await install(k.args, choice)
  }

  const catalog = $derived(query.data?.catalog)
  const catalogs = $derived(catalog?.catalogs ?? [])
  /** The enabled catalogs that could not be read, by name and reason. */
  const unreadable = $derived(catalogs.filter((c) => c.enabled && c.error).map((c) => `${c.name}: ${c.error}`))
  /** Why Discover lists nothing, in the operator's terms: no catalog at
   *  all, every catalog off, the catalogs could not be read, or they are
   *  simply empty. */
  const emptyWhy = $derived.by(() => {
    if (catalogs.length === 0) return { title: 'No catalog configured', hint: 'Add a catalog below, or install any app from its git URL' }
    if (!catalogs.some((c) => c.enabled)) return { title: 'Every catalog is off', hint: 'Turn one on below, or install any app from its git URL' }
    if (unreadable.length > 0) return { title: 'Catalogs unreachable', hint: `${unreadable.join('; ')}. You can still install any app from its git URL below` }
    return { title: 'No apps in the catalogs yet', hint: 'Install any app from its git URL below' }
  })
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <div class="font-sans text-xs text-muted">
      Community and official apps from the catalogs this instance lists.
      <InfoTip text="Installing an app clones its repository into this deployment and its code runs fully trusted, like the platform itself. Install only apps you trust. Official apps are maintained by Outcrop Labs." />
    </div>
    <button
      onclick={() => void query.refetch()}
      class={cn('flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg', query.isFetching && 'gd-breathe')}
    >
      <RefreshCw size={12} /> Refresh
    </button>
  </div>

  {#if notice}<div class="rounded-lg border border-line bg-card px-4 py-3 font-sans text-xs text-fg">{notice}</div>{/if}
  {#if keptConflict}
    <!-- Kept data from another repository: never adopted silently. -->
    <div class="space-y-2 rounded-lg border border-line bg-card px-4 py-3" data-kept-data-conflict>
      <div class="font-sans text-xs text-fg">
        Not installed. The slug <code>{keptConflict.conflict.slug}</code> holds data kept from
        {#if keptConflict.conflict.keptFrom}<code class="break-all">{keptConflict.conflict.keptFrom}</code>{:else}an earlier install that did not record its repository{/if},
        and this install is <code class="break-all">{keptConflict.conflict.installing}</code>.
      </div>
      <div class="flex flex-wrap gap-2">
        <Button size="sm" disabled={busy !== null} onclick={() => void settleKept('adopt')}>Install, adopting that data</Button>
        <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void settleKept('wipe')}>Delete that data, then install</Button>
        <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => (keptConflict = null)}>Cancel</Button>
      </div>
    </div>
  {/if}

  {#if query.isLoading}
    <SkeletonRows rows={4} />
  {:else if !query.data}
    <!-- Distinct from "Catalogs unreachable" below: THAT is the server
         telling us it could not read a catalog (a real, reported answer,
         with each catalog's `error` to quote). This is OUR read of the
         server failing, which says nothing about the marketplace at all. -->
    <QueryError error={query.error} title="Could not load apps" onRetry={() => void query.refetch()} />
  {:else if (catalog?.apps ?? []).length === 0}
    <EmptyState icon="◎" title={emptyWhy.title} hint={emptyWhy.hint} />
  {:else}
    <!-- data-app-cards: the hook Apps.svelte's data-stagger-items selector
         targets — these cards cascade on the page entrance. -->
    <div class="grid gap-3 sm:grid-cols-2" data-app-cards>
      {#each catalog!.apps as c (c.slug)}
        <div class="flex flex-col gap-2 rounded-lg border border-line bg-panel p-4">
          <div class="flex items-center gap-3">
            <span class="text-2xl text-accent">{c.icon}</span>
            <div class="min-w-0 flex-1">
              <div class="flex items-baseline gap-2">
                <span class="truncate font-sans text-sm font-medium text-fg">{c.name}</span>
                {#if c.official}<Chip tone="accent">official</Chip>{/if}
              </div>
              <div class="truncate font-mono text-[10px] tracking-[0.05em] text-muted" title={`Listed by the ${c.catalog.name} catalog`}>by {c.author} · {c.catalog.name}</div>
            </div>
          </div>
          <div class="flex-1 font-sans text-xs text-muted">{c.description}</div>
          <div class="flex items-center justify-between">
            <a href={c.repo} target="_blank" rel="noreferrer" class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted underline-offset-2 hover:underline">
              source
            </a>
            {#if installed.has(c.slug)}
              <Chip>installed</Chip>
            {:else if isAdmin}
              <Button size="sm" disabled={busy !== null} onclick={() => void install({ url: c.repo, slug: c.slug })}>
                <Download size={12} /> Install
              </Button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if query.data && catalog}
    <AppsCatalogs {catalogs} {isAdmin} />
  {/if}

  {#if isAdmin}
    <div class="rounded-lg border border-line bg-panel p-4">
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Install from Git</div>
      <p class="mb-3 font-sans text-xs text-muted">
        Any https git repository with a <code class="text-fg">talaria.json</code> at its root. A private repository on one of
        this instance's git hosts (Admin → Git hosts) is read with that host's login — read-only, where the host can scope
        one; a private repository anywhere else needs a workspace secret that holds a token and lists that host. The code becomes part of this deployment and runs fully trusted, so
        install only repositories you trust.
      </p>
      <div class="flex flex-wrap gap-2">
        <Input
          bind:value={gitUrl}
          placeholder="https://github.com/you/talaria-app-yourthing"
          class="min-w-64 flex-1"
        />
        <Input bind:value={gitRef} placeholder="ref (default branch)" class="w-40" />
        <Input bind:value={gitSecret} placeholder="workspace secret (optional)" class="w-52" />
        <Button
          disabled={!/^(https|http|file):\/\/.+/.test(gitUrl.trim()) || busy !== null}
          onclick={() => void install({ url: gitUrl.trim(), ref: gitRef.trim(), secret: gitSecret.trim() })}
        >
          Install
        </Button>
      </div>
    </div>
  {/if}
</div>

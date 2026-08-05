<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Download, RefreshCw } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { cn } from '@/lib/cn'
  import { fetchAdminApps, post } from './apps'

  let { isAdmin }: { isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['admin-apps-catalog'],
    queryFn: () => fetchAdminApps(true),
    staleTime: 5 * 60_000,
  }))
  let gitUrl = $state('')
  let busy = $state<string | null>(null)
  let notice = $state<string | null>(null)

  const installed = $derived(new Set((query.data?.apps ?? []).map((a) => a.slug).concat(query.data?.pending ?? [])))

  const install = async (url: string, slug?: string) => {
    busy = slug ?? url
    notice = null
    try {
      const r = await post({ installUrl: url, ...(slug ? { slug } : {}) })
      if (r.error) {
        void alert({ title: 'Install failed', message: r.error })
      } else {
        notice = r.pendingBuild
          ? `Installed apps/${r.slug}. It compiles into the next build — reload the dev server (or rebuild) then enable it under Installed.`
          : `Installed apps/${r.slug} — enable it under Installed.`
        gitUrl = ''
        await qc.invalidateQueries({ queryKey: ['admin-apps'] })
        await qc.invalidateQueries({ queryKey: ['admin-apps-catalog'] })
      }
    } finally {
      busy = null
    }
  }

  const catalog = $derived(query.data?.catalog)
</script>

<div class="space-y-6">
  <div class="flex items-center justify-between">
    <div class="font-sans text-xs text-muted">
      Community and official apps from the marketplace index.
      <InfoTip text="Installing an app clones its repository into this deployment and its code runs fully trusted, like the platform itself. Install only apps you trust — official apps are maintained by Outcrop Labs." />
    </div>
    <button
      onclick={() => void query.refetch()}
      class={cn('flex items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg', query.isFetching && 'gd-breathe')}
    >
      <RefreshCw size={12} /> Refresh
    </button>
  </div>

  {#if notice}<div class="rounded-lg border border-line bg-card px-4 py-3 font-sans text-xs text-fg">{notice}</div>{/if}

  {#if query.isLoading}
    <SkeletonRows rows={4} />
  {:else if !query.data}
    <!-- Distinct from "Marketplace unreachable" below: THAT is the server
         telling us it could not reach the catalog feed (a real, reported
         answer, with `catalog.error` to quote). This is OUR read of the
         server failing, which says nothing about the marketplace at all. -->
    <QueryError error={query.error} title="Could not load apps" onRetry={() => void query.refetch()} />
  {:else if (catalog?.apps ?? []).length === 0}
    <EmptyState
      icon="◎"
      title="Marketplace unreachable"
      hint={catalog?.error ? `${catalog.error} — you can still install any app from its git URL below` : 'No apps in the catalog yet — install from a git URL below'}
    />
  {:else}
    <div class="grid gap-3 sm:grid-cols-2">
      {#each catalog!.apps as c (c.slug)}
        <div class="flex flex-col gap-2 rounded-lg border border-line bg-panel p-4">
          <div class="flex items-center gap-3">
            <span class="text-2xl text-accent">{c.icon}</span>
            <div class="min-w-0 flex-1">
              <div class="flex items-baseline gap-2">
                <span class="truncate font-sans text-sm font-medium text-fg">{c.name}</span>
                {#if c.official}<Chip tone="accent">official</Chip>{/if}
              </div>
              <div class="font-mono text-[10px] tracking-[0.05em] text-muted">by {c.author}</div>
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
              <Button size="sm" disabled={busy !== null} onclick={() => void install(c.repo, c.slug)}>
                <Download size={12} /> Install
              </Button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}

  {#if isAdmin}
    <div class="rounded-lg border border-line bg-panel p-4">
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Install from Git</div>
      <p class="mb-3 font-sans text-xs text-muted">
        Any https git repository with a <code class="text-fg">talaria.json</code> at its root. The code becomes part of this
        deployment and runs fully trusted — install only repositories you trust.
      </p>
      <div class="flex gap-2">
        <Input
          bind:value={gitUrl}
          placeholder="https://github.com/you/talaria-app-yourthing"
          class="flex-1"
        />
        <Button disabled={!/^https:\/\/.+/.test(gitUrl.trim()) || busy !== null} onclick={() => void install(gitUrl.trim())}>
          Install
        </Button>
      </div>
    </div>
  {/if}
</div>

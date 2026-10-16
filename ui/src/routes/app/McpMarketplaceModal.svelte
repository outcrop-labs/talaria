<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Check, Plus, RefreshCw } from '@lucide/svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import McpCredentialsModal from './McpCredentialsModal.svelte'
  import McpInstallDialog from './McpInstallDialog.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import {
    TIER_BADGE,
    catalogLine,
    connectPopup,
    emptyHint,
    installLibraryServer,
    syncCatalog,
    useMcpLibrary,
    useMcpServers,
    type InstallOpts,
    type KindFilter,
    type LibraryServerRow,
    type McpServerRow,
    type TierFilter,
  } from './mcp'

  /** The marketplace: the official MCP registry, mirrored locally and
   *  searched here — featured first-party servers up front, full-text search
   *  across the whole catalog, one-click install. Installing runs discovery
   *  in the same step, so a server that needs a key asks for it right here
   *  and a server that speaks OAuth offers its Connect right here. Custom /
   *  self-hosted endpoints live in the separate form.
   *
   *  Two callers: Manage → MCP (install into the org registry) and an agent's
   *  MCP tab (`onInstalled` / `onPickInstalled` attach the server to that
   *  agent as well; `carriedServerIds` marks the ones it already has). */
  let {
    onClose,
    onCustom,
    onInstalled,
    onPickInstalled,
    carriedServerIds = [],
    title = 'MCP marketplace',
  }: {
    onClose: () => void
    onCustom: () => void
    onInstalled?: (serverId: string) => void
    onPickInstalled?: (serverId: string) => void
    carriedServerIds?: string[]
    title?: string
  } = $props()

  const qc = useQueryClient()
  const existingQuery = useMcpServers()
  let q = $state('')
  // Debounced: the catalog answers in milliseconds, but every keystroke is
  // still a request — 200ms is below what a person notices.
  let dq = $state('')
  let debounce: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    const next = q
    clearTimeout(debounce)
    debounce = setTimeout(() => (dq = next), 200)
    return () => clearTimeout(debounce)
  })
  let tier = $state<TierFilter>('all')
  let kind = $state<KindFilter>('all')
  let busyAdd = $state<string | null>(null)
  let added = $state<Map<string, { serverId: string; state: 'ok' | 'needs-key' | 'connect' | 'needs-app' | 'failed'; detail?: string }>>(new Map())
  let error = $state<string | null>(null)
  let installing = $state<LibraryServerRow | null>(null)
  let credentialsFor = $state<McpServerRow | null>(null)
  let syncing = $state(false)

  const resultsQuery = useMcpLibrary(() => ({ q: dq, tier, kind }))
  const results = $derived(resultsQuery.data)
  const catalog = $derived(results?.catalog)
  const filtered = $derived(dq.trim() !== '' || tier !== 'all' || kind !== 'all')

  // Installed dedupe: hosted entries by endpoint; package entries by
  // registry identity (their url is a routing token, not an endpoint).
  const existing = $derived(existingQuery.data ?? [])
  const existingByUrl = $derived(new Map(existing.filter((s) => !s.package).map((s) => [s.url, s])))
  const existingByPackage = $derived(new Map(existing.filter((s) => s.package).map((s) => [s.package!.identifier, s])))
  const existingFor = (l: LibraryServerRow): McpServerRow | undefined =>
    l.package ? existingByPackage.get(l.package.identifier) : existingByUrl.get(l.url)
  const carried = $derived(new Set(carriedServerIds))

  const sync = async () => {
    syncing = true
    try {
      await syncCatalog()
    } catch (e) {
      error = errorMessage(e)
    }
    // The walk runs behind the answer; poll the status a few times so the
    // line updates as pages land.
    for (let i = 0; i < 8; i++) {
      await new Promise((r) => setTimeout(r, 2500))
      await qc.invalidateQueries({ queryKey: ['mcp-library'] })
    }
    syncing = false
  }

  const register = async (l: LibraryServerRow, opts: InstallOpts) => {
    busyAdd = l.registryName
    error = null
    try {
      const out = await installLibraryServer(l, opts)
      installing = null
      await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
      const state = out.needsOauthApp ? 'needs-app' : out.needsConnect ? 'connect' : out.needsCredentials ? 'needs-key' : out.discoveryError ? 'failed' : 'ok'
      added = new Map(added).set(l.registryName, { serverId: out.serverId, state, detail: out.discoveryError ?? undefined })
      onInstalled?.(out.serverId)
      // The drop-in moment: a server that answered 401 asks for its key
      // now, not after someone finds the card.
      if (out.needsCredentials) {
        const fresh = (await qc.fetchQuery<McpServerRow[]>({ queryKey: ['mcp-servers'] })).find((s) => s.id === out.serverId)
        if (fresh) credentialsFor = fresh
      }
      return true
    } catch (e) {
      error = errorMessage(e)
      return false
    } finally {
      busyAdd = null
    }
  }

  // Package installs and servers declaring credentials get the install
  // dialog (credentials to type, third-party code to acknowledge); the rest
  // one-click — discovery then says whether a key is needed after all.
  const install = (l: LibraryServerRow) => {
    if (l.package || l.requiredHeaders.length > 0) installing = l
    else void register(l, {})
  }

  const TIERS: Array<{ id: TierFilter; label: string }> = [
    { id: 'all', label: 'All' },
    { id: 'official', label: 'Official' },
    { id: 'verified', label: 'Verified' },
    { id: 'community', label: 'Community' },
  ]
  const KINDS: Array<{ id: KindFilter; label: string }> = [
    { id: 'all', label: 'Any' },
    { id: 'hosted', label: 'Hosted' },
    { id: 'package', label: 'Packages' },
  ]
</script>

{#snippet chip(active: boolean, label: string, onclick: () => void)}
  <button
    type="button"
    {onclick}
    class={cn(
      'rounded border px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
      active ? 'border-accent/50 bg-accent/10 text-accent' : 'border-line-subtle text-muted hover:border-line hover:text-fg',
    )}
  >
    {label}
  </button>
{/snippet}

<Modal open {onClose} {title} takeover>
  <div class="flex h-full min-h-0 flex-col gap-3">
    <div class="flex items-center gap-3">
      <Input
        autofocus
        bind:value={q}
        placeholder="Search the catalog: GitHub, Linear, Notion, Stripe, Google Drive…"
        class="max-w-xl"
      />
      {#if resultsQuery.isFetching}<WaitingMark site="mcp/search" size={12} class="text-muted" />{/if}
      <span class="flex-1"></span>
      <button type="button" onclick={onCustom} class="text-xs text-accent hover:underline">
        Custom / self-hosted server →
      </button>
    </div>
    <div class="flex flex-wrap items-center gap-x-4 gap-y-1.5">
      <div class="flex items-center gap-1">{#each TIERS as t (t.id)}{@render chip(tier === t.id, t.label, () => (tier = t.id))}{/each}</div>
      <div class="flex items-center gap-1">{#each KINDS as k (k.id)}{@render chip(kind === k.id, k.label, () => (kind = k.id))}{/each}</div>
      <span class="flex-1"></span>
      <span class="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        {#if catalog}
          <span title={catalog.error ?? undefined}>{catalogLine(catalog, relativeTime)}</span>
          {#if catalog.phase !== 'ready' || catalog.error}
            <Button size="xs" variant="ghost" class="gap-1 normal-case tracking-normal" disabled={syncing || catalog.syncing} onclick={() => void sync()}>
              {#if syncing || catalog.syncing}<WaitingMark site="mcp/sync" size={11} />{:else}<RefreshCw size={11} />{/if}
              Sync now
            </Button>
          {/if}
        {/if}
        <InfoTip text="The official MCP registry, mirrored into this instance and refreshed every half hour. Search covers titles, publishers, names and descriptions — hosted endpoints and runnable packages alike." />
      </span>
    </div>
    {#if !filtered}
      <div class="flex items-center gap-1.5">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Featured</span>
        <InfoTip text="Companies publishing their own MCP server on their own verified domain: the official integrations. Search reaches the whole catalog, community servers included." />
      </div>
    {/if}
    {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
    <div class="min-h-0 flex-1 overflow-y-auto">
      {#if resultsQuery.isError}
        <EmptyState
          icon="⚠"
          title="Marketplace didn't load"
          hint={errorMessage(resultsQuery.error)}
        />
      {:else if !results}
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {#each Array.from({ length: 9 }, (_, i) => i) as i (i)}
            <div class="rounded-lg border border-line-subtle bg-surface p-4">
              <Skeleton class="h-3.5 w-32 rounded-full" />
              <Skeleton class="mt-2 h-2.5 w-24 rounded-full" />
              <Skeleton class="mt-3 h-2.5 w-full rounded-full" />
              <Skeleton class="mt-1.5 h-2.5 w-4/5 rounded-full" />
            </div>
          {/each}
        </div>
      {:else if results.servers.length === 0}
        {@const e = emptyHint(catalog, dq)}
        <EmptyState icon="⌁" title={e.title} hint={e.hint}>
          {#snippet action()}
            {#if catalog && catalog.phase === 'empty' && !catalog.syncing}
              <Button size="sm" disabled={syncing} onclick={() => void sync()}>Sync the catalog</Button>
            {:else}
              <Button size="sm" variant="outline" onclick={onCustom}>Register a custom server</Button>
            {/if}
          {/snippet}
        </EmptyState>
      {:else}
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {#each results.servers as l (l.registryName)}
            {@const have = existingFor(l)}
            {@const addedState = added.get(l.registryName)}
            {@const installed = !!addedState || !!have}
            {@const serverId = addedState?.serverId ?? have?.id}
            {@const onAgent = !!serverId && carried.has(serverId)}
            {@const busy = busyAdd === l.registryName}
            {@const badge = TIER_BADGE[l.tier]}
            <div
              class="group relative flex flex-col rounded-lg border border-line-subtle p-4 transition-colors hover:border-line dither-fill"
            >
              <div class="flex items-center gap-3 pr-8">
                <McpServerMark title={l.title} domain={l.domain} icon={l.icon} size={34} />
                <div class="min-w-0 flex-1">
                  <div class="truncate font-sans text-sm font-semibold text-fg">{l.title}</div>
                  <div class="flex flex-wrap items-center gap-1.5">
                    {#if l.domain}<span class="truncate font-mono text-[11px] text-muted">{l.domain}</span>{/if}
                    <span title={badge.hint} class={cn('shrink-0 rounded border px-1.5 font-mono text-[10px] uppercase tracking-[0.05em]', badge.cls)}>
                      {badge.label}
                    </span>
                    {#if l.package}
                      <span
                        title="Ships as a package (npm/pypi/docker) — Talaria runs it in a hardened container"
                        class="shrink-0 rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim"
                      >
                        pkg
                      </span>
                    {/if}
                    {#if l.requiredHeaders.length || l.package?.declaredEnv.length}
                      <span
                        title="Declares the credentials it needs — you'll be asked for them"
                        class="shrink-0 rounded border border-line-subtle px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim"
                      >
                        key
                      </span>
                    {/if}
                  </div>
                </div>
              </div>
              <p class="mt-2.5 line-clamp-3 min-h-12 font-sans text-xs leading-relaxed text-muted">
                {l.description ?? 'No description.'}
              </p>
              {#if addedState && addedState.state !== 'ok'}
                <div class="mt-2 flex flex-wrap items-center gap-2 font-sans text-[11px]">
                  {#if addedState.state === 'connect'}
                    <span class="text-muted">Added. It uses OAuth —</span>
                    <Button size="xs" variant="accent-soft" onclick={() => void connectPopup(addedState.serverId, 'org')}>Connect org account</Button>
                  {:else if addedState.state === 'needs-app'}
                    <span class="text-warning">Added. Needs a one-time OAuth app setup on the MCP page before anyone can connect.</span>
                  {:else if addedState.state === 'needs-key'}
                    <span class="text-warning">Added. It wants credentials —</span>
                    <Button
                      size="xs"
                      variant="accent-soft"
                      onclick={() => {
                        const s = existing.find((x) => x.id === addedState.serverId)
                        if (s) credentialsFor = s
                      }}
                    >
                      Set credentials
                    </Button>
                  {:else}
                    <span class="text-danger" title={addedState.detail}>Added, but discovery failed: {addedState.detail}</span>
                  {/if}
                </div>
              {/if}
              <!-- The install control: quiet until the card is engaged. -->
              <div class="absolute right-3 top-3">
                {#if installed && onPickInstalled && !onAgent && serverId}
                  <Button size="xs" variant="outline" onclick={() => onPickInstalled(serverId)} title="Already in the org registry — attach it to this agent">
                    Use
                  </Button>
                {:else if installed}
                  <span
                    class="grid h-7 w-7 place-items-center rounded-full bg-success/10 text-success"
                    title={onPickInstalled ? 'Already on this agent' : 'In your org registry'}
                  >
                    <Check size={14} />
                  </span>
                {:else}
                  <button
                    type="button"
                    title={l.package ? 'Add to your org (runs a container)' : l.requiredHeaders.length ? 'Add to your org (needs credentials)' : 'Add to your org'}
                    disabled={busy}
                    onclick={() => install(l)}
                    class={cn(
                      'grid h-7 w-7 place-items-center rounded-full border transition-all',
                      busy
                        ? 'border-line bg-raised text-muted opacity-100'
                        : 'border-line bg-raised text-muted opacity-0 hover:border-accent hover:text-accent group-hover:opacity-100',
                    )}
                  >
                    {#if busy}<WaitingMark site="mcp/install" size={12} />{:else}<Plus size={14} />{/if}
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
    {#if installing}
      {@const cur = installing}
      <McpInstallDialog
        server={cur}
        busy={busyAdd === cur.registryName}
        onCancel={() => (installing = null)}
        onInstall={(opts) => void register(cur, opts)}
      />
    {/if}
    {#if credentialsFor}
      {@const cur = credentialsFor}
      <McpCredentialsModal
        server={cur}
        onClose={() => (credentialsFor = null)}
        onSaved={() => {
          // The card's "needs key" note clears once discovery succeeds.
          const entry = Array.from(added.entries()).find(([, v]) => v.serverId === cur.id)
          if (entry) added = new Map(added).set(entry[0], { serverId: cur.id, state: 'ok' })
        }}
      />
    {/if}
  </div>
</Modal>

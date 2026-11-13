<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { RefreshCw } from '@lucide/svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { useIsAdmin, useSession } from '@/lib/session'
  import { useUiVoice } from '@/lib/ui-voice'
  import McpCredentialsModal from './McpCredentialsModal.svelte'
  import McpFeaturedEditor from './McpFeaturedEditor.svelte'
  import McpInstallDialog from './McpInstallDialog.svelte'
  import McpMarketplaceCard from './McpMarketplaceCard.svelte'
  import {
    catalogLine,
    emptyHint,
    hiddenDeadLine,
    installLibraryServer,
    livenessLine,
    showDeadLabel,
    syncCatalog,
    useMcpLibrary,
    useMcpServers,
    type InstallOpts,
    type KindFilter,
    type LibraryServerRow,
    type MarketplaceAdded,
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
    title,
  }: {
    onClose: () => void
    onCustom: () => void
    onInstalled?: (serverId: string) => void
    onPickInstalled?: (serverId: string) => void
    carriedServerIds?: string[]
    title?: string
  } = $props()

  // An agent's dialog opens this from comms too, where the voice is mono:
  // every `sans` fork below keeps that look as it was.
  const sans = useUiVoice() === 'sans'
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
  let added = $state<Map<string, MarketplaceAdded>>(new Map())
  let error = $state<string | null>(null)
  let installing = $state<LibraryServerRow | null>(null)
  let credentialsFor = $state<McpServerRow | null>(null)
  let syncing = $state(false)
  // Unreachable servers (the liveness job's `dead`) are left out unless the
  // person asks; a new query starts hidden again.
  let includeDead = $state(false)
  $effect(() => {
    void [dq, tier, kind]
    includeDead = false
  })
  let editingFeatured = $state(false)
  const sessionQuery = useSession()
  const isAdmin = $derived(useIsAdmin(sessionQuery.data))

  const resultsQuery = useMcpLibrary(() => ({ q: dq, tier, kind, includeDead }))
  const results = $derived(resultsQuery.data)
  const catalog = $derived(results?.catalog)
  const filtered = $derived(dq.trim() !== '' || tier !== 'all' || kind !== 'all')
  const hiddenDead = $derived(hiddenDeadLine(results?.hiddenDead))
  const shownDead = $derived(includeDead && (results?.servers ?? []).some((s) => s.live?.state === 'dead'))

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

  const editCredentials = (serverId: string) => {
    const s = existing.find((x) => x.id === serverId)
    if (s) credentialsFor = s
  }
</script>

{#snippet chip(active: boolean, label: string, onclick: () => void)}
  <button
    type="button"
    {onclick}
    class={cn(
      'rounded border px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors', // voice-ok: mono branch
      active ? 'border-accent/50 bg-accent/10 text-accent' : 'border-line-subtle text-muted hover:border-line hover:text-fg',
    )}
  >
    {label}
  </button>
{/snippet}

{#snippet syncButton()}
  {#if catalog && (catalog.phase !== 'ready' || catalog.error)}
    <Button size="xs" variant="ghost" class={cn('gap-1', !sans && 'normal-case tracking-normal')} disabled={syncing || catalog.syncing} onclick={() => void sync()}>
      {#if syncing || catalog.syncing}<WaitingMark site="mcp/sync" size={11} />{:else}<RefreshCw size={sans ? 12 : 11} />{/if}
      Sync now
    </Button>
  {/if}
{/snippet}

<Modal open {onClose} title={title ?? (sans ? 'Marketplace' : 'MCP marketplace')} takeover>
  <div class={cn('flex h-full min-h-0 flex-col', sans ? 'gap-4' : 'gap-3')}>
    {#if sans}
      <div class="flex flex-wrap items-center gap-3">
        <Input autofocus size="sm" bind:value={q} placeholder="Search servers, such as GitHub, Linear or Notion" class="w-[360px]" />
        <Segmented options={TIERS} value={tier} onChange={(id) => (tier = id)} />
        <Segmented options={KINDS} value={kind} onChange={(id) => (kind = id)} />
        {#if resultsQuery.isFetching}<WaitingMark site="mcp/search" size={12} class="text-muted" />{/if}
        <span class="flex-1"></span>
        <Button size="sm" variant="ghost" onclick={onCustom}>Custom / self-hosted server</Button>
      </div>
      <div class="flex min-h-7 flex-wrap items-center gap-x-3 gap-y-1">
        {#if !filtered}
          <h3 class="font-sans text-[13px] font-medium leading-5 text-fg">Featured</h3>
          <InfoTip text="Companies that publish their own server on their own verified domain. Search reaches the whole catalog, community servers included." />
          {#if isAdmin}<Button size="xs" variant="ghost" onclick={() => (editingFeatured = true)}>Edit featured</Button>{/if}
        {:else}
          <h3 class="font-sans text-[13px] font-medium leading-5 text-fg">Results</h3>
        {/if}
        {#if hiddenDead}
          <span class="font-sans text-xs leading-4 text-muted">{hiddenDead}</span>
          <Button size="xs" variant="ghost" onclick={() => (includeDead = true)}>Show</Button>
        {:else if shownDead}
          <span class="font-sans text-xs leading-4 text-muted">Unreachable servers are shown last</span>
          <Button size="xs" variant="ghost" onclick={() => (includeDead = false)}>Hide them</Button>
        {/if}
        <span class="flex-1"></span>
        {#if catalog}
          {@const live = livenessLine(catalog.liveness)}
          <!-- `catalogLine` is shared with the mono voice and joins with a long dash. -->
          <!-- The reachability counts are a diagnostic: one hover down. -->
          <span class="font-sans text-xs leading-4 text-muted" title={catalog.error ?? (live || undefined)}>{catalogLine(catalog, relativeTime).replace(' — ', ': ')}</span>
          {@render syncButton()}
        {/if}
        <InfoTip text="The official MCP registry, copied into this instance and refreshed every half hour. Hosted servers are checked every few hours, and the ones that do not answer are hidden until you ask for them." />
      </div>
      {#if error}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
    {:else}
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
        <span class="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim"><!-- voice-ok: mono branch -->
          {#if catalog}
            {@const live = livenessLine(catalog.liveness)}
            <span title={catalog.error ?? undefined}>{catalogLine(catalog, relativeTime)}{live ? ` · ${live}` : ''}</span>
            {@render syncButton()}
          {/if}
          <InfoTip text="The official MCP registry, mirrored into this instance and refreshed every half hour. Search covers titles, publishers, names and descriptions — hosted endpoints and runnable packages alike. Hosted endpoints are asked an MCP initialize every few hours; the ones that don't answer are hidden until you ask for them." />
        </span>
      </div>
      {#if !filtered}
        <div class="flex items-center gap-1.5">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Featured</span><!-- voice-ok: mono branch -->
          <InfoTip text="Companies publishing their own MCP server on their own verified domain: the official integrations. Search reaches the whole catalog, community servers included." />
          {#if isAdmin}
            <button type="button" onclick={() => (editingFeatured = true)} class="ml-1 text-xs text-accent hover:underline">Edit featured</button>
          {/if}
        </div>
      {/if}
      {#if hiddenDead || shownDead}
        <div class="flex items-center gap-1.5 font-sans text-xs text-muted">
          {#if hiddenDead}
            <span>{hiddenDead}</span>
            <span aria-hidden="true">·</span>
            <button type="button" onclick={() => (includeDead = true)} class="text-accent hover:underline">show</button>
          {:else}
            <span>Showing unreachable servers last</span>
            <span aria-hidden="true">·</span>
            <button type="button" onclick={() => (includeDead = false)} class="text-accent hover:underline">hide them</button>
          {/if}
        </div>
      {/if}
      {#if error}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{error}</div>{/if}
    {/if}
    <div class="min-h-0 flex-1 overflow-y-auto">
      {#if resultsQuery.isError}
        <EmptyState
          icon="⚠"
          title={sans ? 'The marketplace did not load' : "Marketplace didn't load"}
          hint={errorMessage(resultsQuery.error)}
        />
      {:else if !results}
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {#each Array.from({ length: 9 }, (_, i) => i) as i (i)}
            <div class={cn('rounded-lg border p-4', sans ? 'border-line' : 'border-line-subtle bg-surface')}>
              <Skeleton class="h-3.5 w-32 rounded-full" />
              <Skeleton class="mt-2 h-2.5 w-24 rounded-full" />
              <Skeleton class="mt-3 h-2.5 w-full rounded-full" />
              <Skeleton class="mt-1.5 h-2.5 w-4/5 rounded-full" />
            </div>
          {/each}
        </div>
      {:else if results.servers.length === 0 && hiddenDead}
        <EmptyState
          icon="⌁"
          title="Only unreachable servers match"
          hint={sans
            ? 'Every match failed its last check. You can still add one if you know it is back.'
            : "Every match failed its last liveness check — its endpoint 404'd, errored or didn't answer. They can still be installed if you know they're back."}
        >
          {#snippet action()}
            <Button size="sm" variant="outline" onclick={() => (includeDead = true)}>{showDeadLabel(results.hiddenDead)}</Button>
          {/snippet}
        </EmptyState>
      {:else if results.servers.length === 0}
        {@const e = emptyHint(catalog, dq)}
        <EmptyState icon="⌁" title={e.title} hint={e.hint}>
          {#snippet action()}
            {#if catalog && catalog.phase === 'empty' && !catalog.syncing}
              <Button size="sm" variant={sans ? 'outline' : 'primary'} disabled={syncing} onclick={() => void sync()}>Sync the catalog</Button>
            {:else}
              <Button size="sm" variant="outline" onclick={onCustom}>{sans ? 'Add a custom server' : 'Register a custom server'}</Button>
            {/if}
          {/snippet}
        </EmptyState>
      {:else}
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {#each results.servers as l (l.registryName)}
            {@const addedState = added.get(l.registryName)}
            {@const serverId = addedState?.serverId ?? existingFor(l)?.id}
            <McpMarketplaceCard
              entry={l}
              added={addedState}
              {serverId}
              onAgent={!!serverId && carried.has(serverId)}
              busy={busyAdd === l.registryName}
              onInstall={() => install(l)}
              onPick={onPickInstalled}
              onSetCredentials={editCredentials}
            />
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
    {#if editingFeatured}
      <McpFeaturedEditor onClose={() => (editingFeatured = false)} />
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

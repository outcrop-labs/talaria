<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Check, Plus } from '@lucide/svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { getJson } from '@/lib/fetch-json'
  import McpInstallDialog from './McpInstallDialog.svelte'
  import McpServerMark from './McpServerMark.svelte'
  import { TIER_BADGE, patchServer, slugify, useMcpServers, type LibraryServerRow } from './mcp'

  /** The marketplace: browse the official MCP registry like a store — featured
   *  first-party servers up front, full search across 20k+ entries, one-click
   *  install. Custom/self-hosted endpoints live in the separate form. */
  let { onClose, onCustom }: { onClose: () => void; onCustom: () => void } = $props()

  const qc = useQueryClient()
  const existingQuery = useMcpServers()
  let q = $state('')
  let busyAdd = $state<string | null>(null)
  let added = $state<Map<string, 'ok' | 'setup'>>(new Map())
  let error = $state<string | null>(null)
  let installing = $state<LibraryServerRow | null>(null)

  const resultsQuery = createQuery(() => ({
    queryKey: ['mcp-marketplace', q],
    queryFn: (): Promise<{ servers: LibraryServerRow[] }> =>
      getJson<{ servers: LibraryServerRow[] }>(
        q.trim() ? `/api/mcp/library?q=${encodeURIComponent(q.trim())}` : '/api/mcp/library?featured=1',
      ),
    placeholderData: (prev: { servers: LibraryServerRow[] } | undefined) => prev,
    staleTime: 60_000,
  }))
  const results = $derived(resultsQuery.data)

  const installedUrls = $derived(new Set((existingQuery.data ?? []).map((s) => s.url)))

  const register = async (l: LibraryServerRow, opts: { headers?: Record<string, string>; authMode?: 'org' | 'per-user' }) => {
    busyAdd = l.registryName
    error = null
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
    busyAdd = null
    if (!r.ok) {
      error = ((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed'
      return false
    }
    const { server } = (await r.json()) as { server: { id: string; oauthMeta: { dcr: boolean; clientSet: boolean } | null } }
    const needsSetup = !!server.oauthMeta && !server.oauthMeta.dcr && !server.oauthMeta.clientSet
    added = new Map(added).set(l.registryName, needsSetup ? 'setup' : 'ok')
    installing = null
    await qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    void patchServer(server.id, { refreshTools: true }).then(() => qc.invalidateQueries({ queryKey: ['mcp-servers'] }))
    return true
  }

  // Servers declaring credentials get the install dialog; the rest one-click.
  const install = (l: LibraryServerRow) => {
    if (l.requiredHeaders.length > 0) installing = l
    else void register(l, {})
  }
</script>

<Modal open {onClose} title="MCP marketplace" takeover>
  <div class="flex h-full min-h-0 flex-col gap-4">
    <div class="flex items-center gap-3">
      <Input
        autofocus
        bind:value={q}
        placeholder="Search the official MCP registry — GitHub, Linear, Notion, Stripe, Vercel…"
        class="max-w-xl"
      />
      {#if resultsQuery.isFetching}<GeneratingBars bars={3} variant="scan" class="text-muted" />{/if}
      <span class="flex-1"></span>
      <button type="button" onclick={onCustom} class="text-xs text-accent hover:underline">
        Custom / self-hosted server →
      </button>
    </div>
    {#if !q.trim()}
      <div class="flex items-center gap-1.5">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Featured</span>
        <InfoTip text="Companies publishing their own MCP server on their own verified domain — the official integrations. Search reaches the whole registry, community servers included." />
      </div>
    {/if}
    {#if error}<div class="text-sm text-danger">{error}</div>{/if}
    <div class="min-h-0 flex-1 overflow-y-auto">
      {#if !results}
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {#each Array.from({ length: 9 }, (_, i) => i) as i (i)}
            <div class="rounded-lg border border-line-subtle p-4">
              <Skeleton class="h-3.5 w-32 rounded-full" delay={i * 0.08} />
              <Skeleton class="mt-2 h-2.5 w-24 rounded-full" delay={i * 0.08 + 0.06} />
              <Skeleton class="mt-3 h-2.5 w-full rounded-full" delay={i * 0.08 + 0.12} />
              <Skeleton class="mt-1.5 h-2.5 w-4/5 rounded-full" delay={i * 0.08 + 0.18} />
            </div>
          {/each}
        </div>
      {:else if results.servers.length === 0}
        <EmptyState
          icon="⌁"
          title="No hosted servers match"
          hint="Only servers with a hosted endpoint appear — packages that need a local process can't be one-click added. Try the custom form for self-hosted servers."
        />
      {:else}
        <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {#each results.servers as l (l.registryName)}
            {@const addedState = added.get(l.registryName) ?? (installedUrls.has(l.url) ? 'ok' : null)}
            {@const installed = addedState !== null}
            {@const busy = busyAdd === l.registryName}
            {@const badge = TIER_BADGE[l.tier]}
            <div
              class="group relative flex flex-col rounded-lg border border-line-subtle p-4 transition-colors hover:border-line hover:bg-hover"
            >
              <div class="flex items-center gap-3">
                <McpServerMark title={l.title} domain={l.domain} icon={l.icon} size={34} />
                <div class="min-w-0 flex-1">
                  <div class="truncate font-sans text-sm font-semibold text-fg">{l.title}</div>
                  <div class="flex items-center gap-1.5">
                    {#if l.domain}<span class="truncate font-mono text-[11px] text-muted">{l.domain}</span>{/if}
                    <span title={badge.hint} class={cn('shrink-0 rounded border px-1.5 font-mono text-[10px] uppercase tracking-[0.05em]', badge.cls)}>
                      {badge.label}
                    </span>
                  </div>
                </div>
              </div>
              <p class="mt-2.5 line-clamp-3 min-h-12 font-sans text-xs leading-relaxed text-muted">
                {l.description ?? 'No description.'}
              </p>
              <!-- The install control: quiet until the card is engaged. -->
              <div class="absolute right-3 top-3">
                {#if addedState === 'setup'}
                  <span
                    class="rounded border border-warning/40 px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-warning"
                    title="Added — this provider needs a one-time OAuth app setup on the MCP page before agents can connect"
                  >
                    needs setup
                  </span>
                {:else if installed}
                  <span
                    class="grid h-7 w-7 place-items-center rounded-full bg-success/10 text-success"
                    title="In your org registry"
                  >
                    <Check size={14} />
                  </span>
                {:else}
                  <button
                    type="button"
                    title={l.requiredHeaders.length ? 'Add to your org (needs credentials)' : 'Add to your org'}
                    disabled={busy}
                    onclick={() => install(l)}
                    class={cn(
                      'grid h-7 w-7 place-items-center rounded-full border transition-all',
                      busy
                        ? 'border-line bg-raised text-muted opacity-100'
                        : 'border-line bg-raised text-muted opacity-0 hover:border-accent hover:text-accent group-hover:opacity-100',
                    )}
                  >
                    {#if busy}<GeneratingBars bars={3} variant="weave" step={0.15} />{:else}<Plus size={14} />{/if}
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
  </div>
</Modal>

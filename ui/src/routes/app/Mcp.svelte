<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { navigate, route } from '@/router'
  import { tabFromPath } from '@/lib/route-tabs'
  import McpAddServerModal from './McpAddServerModal.svelte'
  import McpMarketplaceModal from './McpMarketplaceModal.svelte'
  import McpServerCard from './McpServerCard.svelte'
  import { isInternalServer, useMcpServers } from './mcp'

  // Manage → MCP: the org's MCP servers in one registry. Register a server
  // once; decide which agents carry it (optionally a tool subset per agent) and
  // which people may exercise it through agents acting for them (optionally
  // their own tool subset). Per-user servers act with each person's CONNECTED
  // ACCOUNT (Settings → Connections). Enforcement lives in the MCP gateway,
  // not in agent configs.
  // Two populations with different rules, and mixing them made one list where
  // half the rows could be repointed and half could not. Internal servers are
  // this deployment's own — you govern access; external ones are third-party
  // endpoints someone registered, and only those can be added or removed.
  //
  // THE URL IS THE TAB: /mcp (built-in) and /mcp/external.
  const TABS = ['internal', 'external'] as const
  type McpTab = (typeof TABS)[number]
  const tab = $derived(tabFromPath(route.pathname, '/mcp', TABS, 'internal'))
  const goTab = (id: McpTab) => {
    if (id === 'internal') void navigate('/mcp')
    else void navigate('/mcp/:tab', { params: { tab: 'external' } })
  }

  const qc = useQueryClient()
  const serversQuery = useMcpServers()
  let adding = $state<null | 'marketplace' | 'custom'>(null)

  // Silhouette widths vary per index so the sketch doesn't look stamped.
  const titleW = ['w-36', 'w-28', 'w-44']
  const descW = ['w-3/5', 'w-2/5', 'w-1/2']
  const pillW = ['w-16', 'w-14', 'w-20', 'w-12', 'w-16']
</script>

<!-- One server card's silhouette (see McpServerCard: header with 36px mark,
     tools pill strip, then the access table) — same Panel frame and spacing,
     so the real card materializes over it without a jump. -->
{#snippet serverSkeleton(i: number)}
  <div aria-hidden="true" class="rounded-lg border border-line bg-panel p-6">
    <div class="flex items-center gap-3">
      <Skeleton class="h-9 w-9 shrink-0 rounded-md" />
      <div class="min-w-0 flex-1 space-y-2">
        <div class="flex items-baseline gap-2">
          <Skeleton class={`h-3.5 rounded-full ${titleW[i % titleW.length]}`} />
          <Skeleton class="h-2.5 w-24 rounded-full" />
        </div>
        <Skeleton class={`h-2.5 rounded-full ${descW[i % descW.length]}`} />
      </div>
      <Skeleton class="h-5 w-20 shrink-0 rounded" />
    </div>
    <div class="mt-3 flex items-center gap-1.5">
      {#each pillW as w, j (j)}
        <Skeleton class={`h-[22px] rounded ${w}`} />
      {/each}
    </div>
    <div class="mt-4 border-t border-line pt-3">
      <Skeleton class="h-2.5 w-12 rounded-full" />
      <div class="mt-3 space-y-2.5">
        <div class="flex items-center gap-3">
          <Skeleton class="h-2.5 w-14 rounded-full" />
          <Skeleton class="h-3 w-40 rounded-full" />
        </div>
        <div class="flex items-center gap-3">
          <Skeleton class="h-2.5 w-14 rounded-full" />
          <Skeleton class="h-3 w-32 rounded-full" />
        </div>
      </div>
    </div>
  </div>
{/snippet}

<!-- The OAuth popup announces completion — refresh connection states live. -->
<svelte:window
  onmessage={(e) => {
    if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
      void qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    }
  }}
/>

<PageSurface>
  <!-- Page content entrance: header row, then the registry region, rise in
       sequence (ANIMATIONS.md). The registry region rises as one child here;
       the card cascade inside it belongs to Materialize's content branch. -->
  <div use:staggerIn class="space-y-6">
    <ViewHeader
      title="MCP"
      info="Model Context Protocol servers, managed org-wide. Register once; choose which agents carry each server and which people may use it, down to individual tools. Agents reach servers only through Talaria's gateway, so the limits here are enforced, not advisory."
    >
      {#snippet actions()}
        <!-- Adding only ever means adding an EXTERNAL server: the internal ones
             arrive with the deployment and cannot be registered by hand. -->
        {#if tab === 'external'}
          <Button size="sm" variant="ghost" onclick={() => (adding = 'custom')}>
            Custom server
          </Button>
          <Button size="sm" onclick={() => (adding = 'marketplace')}>
            <Plus size={14} /> Browse marketplace
          </Button>
        {/if}
      {/snippet}
    </ViewHeader>

    <Tabs
      items={[
        { id: 'internal', label: 'Built-in' },
        { id: 'external', label: 'External' },
      ]}
      value={tab}
      onChange={goTab}
    />

    <!-- Skeleton → content as one motion: card-shaped skeletons materialize
         into the real cards (Materialize grid-stacks the branches, so no
         layout jump). QueryState keeps owning ERROR and EMPTY — its loading
         branch is bypassed in practice because Materialize intercepts the
         in-flight window; the item-shaped snippet it holds only covers the
         paused-fetch edge (offline before first byte).

         EMPTY is asked per TAB rather than of the registry: a deployment with
         only its own toolkit is not an empty registry, but the external tab is
         genuinely empty and should say so — and say the useful thing, which is
         how to add one. -->
    {#key tab}
    <div in:fly={{ y: 6, duration: 200 }}>
    <Materialize loading={serversQuery.isLoading} count={3} class="space-y-6">
      {#snippet skeleton(i)}{@render serverSkeleton(i)}{/snippet}
      <QueryState
        query={serversQuery}
        errorTitle="Could not load the MCP registry"
        isEmpty={(rows) => rows.filter((s) => isInternalServer(s) === (tab === 'internal')).length === 0}
      >
        {#snippet skeleton()}{@render serverSkeleton(0)}{/snippet}
        {#snippet empty()}
          {#if tab === 'external'}
            <EmptyState
              icon="⌁"
              title="No external servers yet"
              hint="Browse the marketplace, or register a custom endpoint your agents should reach. Talaria's own tools are on the Built-in tab and are already available."
            >
              {#snippet action()}
                <Button size="sm" onclick={() => (adding = 'marketplace')}>Browse marketplace</Button>
              {/snippet}
            </EmptyState>
          {:else}
            <!-- Reachable only if the toolkit row failed to seed, which is a
                 real fault rather than a normal state — so it does not offer an
                 action it cannot deliver. -->
            <EmptyState
              icon="⌁"
              title="Built-in tools unavailable"
              hint="Talaria's own toolkit should always be registered here. If this persists, check the app logs. The MCP service may not have started."
            />
          {/if}
        {/snippet}
        {#snippet children(servers)}
          {#each servers.filter((s) => isInternalServer(s) === (tab === 'internal')) as s (s.id)}
            <McpServerCard server={s} />
          {/each}
        {/snippet}
      </QueryState>
    </Materialize>
    </div>
    {/key}

    {#if adding === 'marketplace'}<McpMarketplaceModal onClose={() => (adding = null)} onCustom={() => (adding = 'custom')} />{/if}
    {#if adding === 'custom'}<McpAddServerModal onClose={() => (adding = null)} />{/if}
  </div>
</PageSurface>

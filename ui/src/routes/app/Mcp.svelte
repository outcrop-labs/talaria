<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import McpAddServerModal from './McpAddServerModal.svelte'
  import McpMarketplaceModal from './McpMarketplaceModal.svelte'
  import McpServerCard from './McpServerCard.svelte'
  import { useMcpServers } from './mcp'

  // Manage → MCP: the org's MCP servers in one registry. Register a server
  // once; decide which agents carry it (optionally a tool subset per agent) and
  // which people may exercise it through agents acting for them (optionally
  // their own tool subset). Per-user servers act with each person's CONNECTED
  // ACCOUNT (Settings → Connections). Enforcement lives in the MCP gateway,
  // not in agent configs.
  const qc = useQueryClient()
  const serversQuery = useMcpServers()
  let adding = $state<null | 'marketplace' | 'custom'>(null)
</script>

<!-- The OAuth popup announces completion — refresh connection states live. -->
<svelte:window
  onmessage={(e) => {
    if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
      void qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    }
  }}
/>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto max-w-5xl space-y-6">
    <div class="flex items-center gap-2">
      <h1 class="font-sans text-2xl font-semibold tracking-tight text-fg">MCP</h1>
      <InfoTip text="Model Context Protocol servers, managed org-wide. Register once; choose which agents carry each server and which people may use it — down to individual tools. Agents reach servers only through Talaria's gateway, so the limits here are enforced, not advisory." />
      <span class="flex-1"></span>
      <Button size="sm" variant="ghost" onclick={() => (adding = 'custom')}>
        Custom server
      </Button>
      <Button size="sm" onclick={() => (adding = 'marketplace')}>
        <Plus size={14} /> Browse marketplace
      </Button>
    </div>

    <QueryState
      query={serversQuery}
      errorTitle="Could not load the MCP registry"
    >
      {#snippet skeleton()}
        <Panel>
          <Skeleton class="mb-3 h-4 w-40 rounded-full" />
          <SkeletonRows rows={3} />
        </Panel>
      {/snippet}
      {#snippet empty()}
        <EmptyState
          icon="⌁"
          title="No MCP servers yet"
          hint="Browse the marketplace, or register a custom endpoint your agents should reach."
        >
          {#snippet action()}
            <Button size="sm" onclick={() => (adding = 'marketplace')}>Browse marketplace</Button>
          {/snippet}
        </EmptyState>
      {/snippet}
      {#snippet children(servers)}
        {#each servers as s (s.id)}
          <McpServerCard server={s} />
        {/each}
      {/snippet}
    </QueryState>

    {#if adding === 'marketplace'}<McpMarketplaceModal onClose={() => (adding = null)} onCustom={() => (adding = 'custom')} />{/if}
    {#if adding === 'custom'}<McpAddServerModal onClose={() => (adding = null)} />{/if}
  </div>
</div>

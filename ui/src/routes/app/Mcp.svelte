<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { fly } from '@/lib/motion'
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
  //
  // The second tab stays "External", not "Connected": it lists servers that
  // are registered but unreachable or waiting for a sign-in, and "Connected"
  // is also one of the words a row's status uses.
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
</script>

<!-- The OAuth popup announces completion — refresh connection states live. -->
<svelte:window
  onmessage={(e) => {
    if (e.origin === window.location.origin && (e.data as { type?: string })?.type === 'talaria:mcp-oauth-done') {
      void qc.invalidateQueries({ queryKey: ['mcp-servers'] })
    }
  }}
/>

<SettingsPage
  title="MCP"
  blurb="Tools your agents can call, and who may use them."
  tabs={[
    { id: 'internal', label: 'Built-in' },
    { id: 'external', label: 'External' },
  ]}
  {tab}
  onTab={goTab}
>
      {#snippet actions()}
        <!-- Adding only ever means adding an EXTERNAL server: the internal ones
             arrive with the deployment and cannot be registered by hand. -->
        {#if tab === 'external'}
          <Button size="sm" variant="ghost" onclick={() => (adding = 'custom')}>Custom server</Button>
          <Button size="sm" onclick={() => (adding = 'marketplace')}>Browse marketplace</Button>
        {/if}
      {/snippet}

    <!-- EMPTY is asked per TAB rather than of the registry: a deployment with
         only its own toolkit is not an empty registry, but the external tab is
         genuinely empty and should say so — and say the useful thing, which is
         how to add one. -->
    {#key tab}
    <div in:fly={{ y: 6, duration: 200 }}>
      <QueryState
        query={serversQuery}
        errorTitle="Could not load your tool servers"
        isEmpty={(rows) => rows.filter((s) => isInternalServer(s) === (tab === 'internal')).length === 0}
      >
        {#snippet skeleton()}
          <SettingsGroup>
            <div class="px-5 py-4"><SkeletonRows rows={4} /></div>
          </SettingsGroup>
        {/snippet}
        {#snippet empty()}
          {#if tab === 'external'}
            <EmptyState
              icon="⌁"
              title="No servers added yet"
              hint="Add a tool server from the marketplace, or your own by its address."
            >
              {#snippet action()}
                <Button size="sm" variant="outline" onclick={() => (adding = 'marketplace')}>Browse marketplace</Button>
              {/snippet}
            </EmptyState>
          {:else}
            <!-- Reachable only if the toolkit row failed to seed, which is a
                 real fault rather than a normal state — so it does not offer an
                 action it cannot deliver. -->
            <EmptyState
              icon="⌁"
              title="Built-in tools are unavailable"
              hint="Talaria's own tools should always be listed here. If this lasts, check the app logs."
            />
          {/if}
        {/snippet}
        {#snippet children(servers)}
          <SettingsGroup>
            {#each servers.filter((s) => isInternalServer(s) === (tab === 'internal')) as s (s.id)}
              <McpServerCard server={s} />
            {/each}
          </SettingsGroup>
        {/snippet}
      </QueryState>
    </div>
    {/key}

    {#if adding === 'marketplace'}<McpMarketplaceModal onClose={() => (adding = null)} onCustom={() => (adding = 'custom')} />{/if}
    {#if adding === 'custom'}<McpAddServerModal onClose={() => (adding = null)} />{/if}
</SettingsPage>

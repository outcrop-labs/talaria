<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { serverStatus } from '@/lib/mcp-status'
  import { useMcpServers } from '@/routes/app/mcp'
  import { p } from '@/router'

  // The MCP servers the agents can reach, and how many need someone: the
  // same verdict (`serverStatus`) each card on the MCP page shows, counted.
  const serversQuery = useMcpServers()
  const servers = $derived(serversQuery.data)
  const enabled = $derived((servers ?? []).filter((s) => s.enabled))
  const attention = $derived(enabled.filter((s) => ['warning', 'danger'].includes(serverStatus(s).tone)).length)
</script>

<StatCard
  label="MCP servers"
  href={p('/mcp')}
  sub={servers
    ? attention > 0
      ? `${attention} need${attention === 1 ? 's' : ''} attention`
      : enabled.length > 0
        ? 'all connected'
        : 'none enabled'
    : serversQuery.isError
      ? 'could not read'
      : undefined}
>
  {#snippet value()}
    {#if servers}{enabled.length}
    {:else if serversQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>

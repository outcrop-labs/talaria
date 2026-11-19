<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import { serverStatus } from '@/lib/mcp-status'
  import { useMcpServers } from '@/routes/app/mcp'
  import { p } from '@/router'

  // The tool servers the agents can reach, and how many need someone: the
  // same verdict (`serverStatus`) each row on the Tools page shows, counted.
  const serversQuery = useMcpServers()
  const servers = $derived(serversQuery.data)
  const enabled = $derived((servers ?? []).filter((s) => s.enabled))
  const attention = $derived(enabled.filter((s) => ['warning', 'danger'].includes(serverStatus(s).tone)).length)
</script>

<Stat
  label="Tool servers"
  href={p('/mcp')}
  sub={servers
    ? attention > 0
      ? `${attention} need${attention === 1 ? 's' : ''} attention`
      : enabled.length > 0
        ? 'All connected'
        : 'None enabled'
    : serversQuery.isError
      ? 'Could not read'
      : undefined}
>
  {#snippet value()}
    {#if servers}{enabled.length}
    {:else if serversQuery.isError}—
    {:else}<Skeleton class="my-1.5 h-5 w-16 rounded-full" />{/if}
  {/snippet}
</Stat>

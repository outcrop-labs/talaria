<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { useTeamsAll } from '@/lib/teams'
  import { p } from '@/router'

  const teamsQuery = useTeamsAll()
  const teams = $derived(teamsQuery.data)
  const people = $derived((teams ?? []).reduce((n, t) => n + t.memberCount, 0))
  const agents = $derived((teams ?? []).reduce((n, t) => n + t.agentCount, 0))
</script>

<StatCard
  label="Teams"
  href={p('/teams')}
  sub={teams ? `${people} member seat${people === 1 ? '' : 's'} · ${agents} agent${agents === 1 ? '' : 's'}` : teamsQuery.isError ? 'could not read' : undefined}
>
  {#snippet value()}
    {#if teams}{teams.length}
    {:else if teamsQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>

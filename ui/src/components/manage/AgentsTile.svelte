<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { useFleetContainers, useFleetDefs } from '@/lib/fleet-defs'
  import { useHasPerm } from '@/lib/session'
  import { healthOf, type Health } from '@/routes/app/agents.svelte'
  import { p } from '@/router'

  // The fleet in one number: how many of the agents you can see are up, and
  // the ones that are not, by kind. Read from the same two queries the
  // Agents roster paints from, so the tile and the roster cannot disagree.
  const defsQuery = useFleetDefs(() => true)
  // The roster is the whole fleet for admins and agents.manage, and only the
  // agents you manage for everyone else — so an empty one means different
  // things, and says which.
  const wholeFleet = useHasPerm('agents.manage')
  const containersQuery = useFleetContainers(() => true)

  const counts = $derived.by(() => {
    const defs = defsQuery.data?.defs
    const containers = containersQuery.data
    if (!defs || !containers) return null
    const byDept = new Map(containers.map((c) => [c.department, c]))
    const tally: Record<Health, number> = { up: 0, warming: 0, degraded: 0, down: 0, draft: 0, retired: 0 }
    for (const d of defs) tally[healthOf(d, byDept.get(d.department) ?? null).health] += 1
    return tally
  })
  const live = $derived(counts ? counts.up + counts.warming + counts.degraded + counts.down : 0)
  const sub = $derived.by(() => {
    if (!counts) return containersQuery.isError ? 'container status unavailable' : undefined
    const parts = [
      counts.down ? `${counts.down} down` : null,
      counts.degraded ? `${counts.degraded} degraded` : null,
      counts.warming ? `${counts.warming} warming` : null,
      counts.draft ? `${counts.draft} draft` : null,
    ].filter(Boolean)
    if (parts.length > 0) return parts.join(' · ')
    if (live > 0) return 'all up'
    return wholeFleet.current ? 'no agents yet' : 'none you manage'
  })
</script>

<StatCard label="Agents up" href={p('/agents')} {sub}>
  {#snippet value()}
    {#if counts}{counts.up}<span class="text-muted">{' / '}{live}</span>
    {:else if defsQuery.isError || containersQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>

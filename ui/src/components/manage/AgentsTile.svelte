<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { anyError } from '@/components/ui/query-state'
  import Stat from '@/components/settings/Stat.svelte'
  import { healthPhrases, rosterCount } from '@/components/fleet/roster-count'
  import { useFleetContainers, useFleetDefs } from '@/lib/fleet-defs'
  import { useHasPerm } from '@/lib/session'
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

  const count = $derived.by(() => {
    const defs = defsQuery.data?.defs
    const containers = containersQuery.data
    if (!defs || !containers) return null
    return rosterCount(defs, new Map(containers.map((c) => [c.department, c])))
  })
  const sub = $derived.by(() => {
    if (!count) return containersQuery.isError ? 'Container status unavailable' : undefined
    // The roster's own exceptions, then its drafts: a draft is a thing to finish, a retired agent is not.
    const parts = [...count.exceptions, ...healthPhrases(count.tally, ['draft'])]
    if (parts.length > 0) return parts.join(' · ')
    if (count.counted > 0) return 'All running'
    return wholeFleet.current ? 'No agents yet' : 'None you manage'
  })
</script>

<Stat label="Agents running" href={p('/agents')} {sub}>
  {#snippet value()}
    {#if count}{count.running}<span class="text-muted">{' / '}{count.counted}</span>
    {:else if anyError(defsQuery, containersQuery)}—
    {:else}<Skeleton class="my-1.5 h-5 w-16 rounded-full" />{/if}
  {/snippet}
</Stat>

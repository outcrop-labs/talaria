<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { formatCost, useCost } from '@/lib/cost.svelte'
  import { p } from '@/router'

  // What the instance spent today, with the month beside it — the same
  // `/api/cost` read the Costs tab breaks down by agent and model.
  const costQuery = useCost()
  const t = $derived(costQuery.data?.totals)
</script>

<StatCard
  label="Spend today"
  href={p('/observability/:tab', { params: { tab: 'cost' } })}
  sub={t ? `${formatCost(t.month.cost ?? 0)} this month` : costQuery.isError ? 'could not read' : undefined}
>
  {#snippet value()}
    {#if t}{formatCost(t.today.cost ?? 0)}
    {:else if costQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>

<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { useTemplates } from '@/lib/templates'
  import { p } from '@/router'

  const templatesQuery = useTemplates()
  const templates = $derived(templatesQuery.data)
  const tickets = $derived((templates ?? []).filter((t) => t.kind === 'ticket').length)
  const plans = $derived((templates ?? []).filter((t) => t.kind === 'plan').length)
</script>

<StatCard
  label="Templates"
  href={p('/templates')}
  sub={templates ? `${tickets} ticket · ${plans} plan` : templatesQuery.isError ? 'could not read' : undefined}
>
  {#snippet value()}
    {#if templates}{templates.length}
    {:else if templatesQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>

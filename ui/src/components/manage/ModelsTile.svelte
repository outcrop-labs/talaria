<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { useEndpoints } from '@/lib/models'
  import { p } from '@/router'

  // Providers, and the two things that stop a model answering: a provider
  // with no key, and a model role pointed at something that is not there.
  // Both reads are admin-only, so the overview mounts this tile for admins.
  const endpointsQuery = useEndpoints(() => true)
  // The same read (and cache key) the Models page's roles panel uses.
  const rolesQuery = createQuery(() => ({
    queryKey: ['model-roles'],
    queryFn: () => getJson<{ issues?: { role: string }[] }>('/api/admin/model-roles'),
  }))
  const endpoints = $derived(endpointsQuery.data)
  const keyless = $derived((endpoints ?? []).filter((e) => e.class === 'cloud' && e.hasKey === false).length)
  const issues = $derived(rolesQuery.data?.issues?.length ?? 0)
  const sub = $derived.by(() => {
    if (!endpoints) return endpointsQuery.isError ? 'Could not read' : undefined
    const parts = [
      keyless ? `${keyless} without a key` : null,
      issues ? `${issues} role${issues === 1 ? '' : 's'} unrouted` : null,
    ].filter(Boolean)
    return parts.length > 0 ? parts.join(' · ') : endpoints.length > 0 ? 'Every role routed' : 'Add a provider'
  })
</script>

<Stat label="Model providers" href={p('/models')} {sub}>
  {#snippet value()}
    {#if endpoints}{endpoints.length}
    {:else if endpointsQuery.isError}—
    {:else}<Skeleton class="my-1.5 h-5 w-16 rounded-full" />{/if}
  {/snippet}
</Stat>

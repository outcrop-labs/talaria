<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import { p } from '@/router'

  // Enabled apps, and any whose build failed — an app with a failed build is
  // installed and enabled but shows nobody anything.
  const appsQuery = useEnabledApps()
  const apps = $derived(appsQuery.data)
  const failed = $derived((apps ?? []).filter((a) => a.build?.status === 'failed').length)
  const building = $derived((apps ?? []).filter((a) => a.build?.status === 'building').length)
</script>

<StatCard
  label="Apps enabled"
  href={p('/apps')}
  sub={apps
    ? failed > 0
      ? `${failed} build${failed === 1 ? '' : 's'} failed`
      : building > 0
        ? `${building} building`
        : apps.length > 0
          ? 'all built'
          : 'none enabled'
    : appsQuery.isError
      ? 'could not read'
      : undefined}
>
  {#snippet value()}
    {#if apps}{apps.length}
    {:else if appsQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>

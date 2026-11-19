<script lang="ts">
  import { useEnabledApps } from '@/lib/apps'
  import { p } from '@/router'
  import InventoryItem from './InventoryItem.svelte'

  // Enabled apps, and any whose build failed: an app with a failed build is
  // installed and enabled but shows nobody anything.
  const appsQuery = useEnabledApps()
  const apps = $derived(appsQuery.data)
  const failed = $derived((apps ?? []).filter((a) => a.build?.status === 'failed').length)
  const building = $derived((apps ?? []).filter((a) => a.build?.status === 'building').length)
</script>

<InventoryItem
  href={p('/apps')}
  count={apps?.length}
  noun="app"
  note={failed > 0 ? `${failed} build${failed === 1 ? '' : 's'} failed` : building > 0 ? `${building} building` : undefined}
  failed={appsQuery.isError}
/>

<script lang="ts">
  import { useTeamsAll } from '@/lib/teams'
  import { p } from '@/router'
  import InventoryItem from './InventoryItem.svelte'

  const teamsQuery = useTeamsAll()
</script>

<InventoryItem href={p('/teams')} count={teamsQuery.data?.length} noun="team" failed={teamsQuery.isError} />

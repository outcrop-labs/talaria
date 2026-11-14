<script lang="ts">
  import { useTemplates } from '@/lib/templates'
  import { p } from '@/router'
  import InventoryItem from './InventoryItem.svelte'

  const templatesQuery = useTemplates()
</script>

<InventoryItem href={p('/templates')} count={templatesQuery.data?.length} noun="template" failed={templatesQuery.isError} />

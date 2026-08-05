<script lang="ts">
  import { navigate } from '@/router'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { useBoards } from '@/lib/boards.svelte'
  import BoardColumnsSkeleton from './BoardColumnsSkeleton.svelte'

  const query = useBoards()
  const firstId = $derived(query.data?.[0]?.id)

  // Open the most recent board automatically (Plane/ClickUp behaviour).
  $effect(() => {
    if (firstId) void navigate('/boards/:boardId', { params: { boardId: firstId }, replace: true })
  })
</script>

<!-- "No boards yet" is now reachable ONLY from a 200 that really carried an
     empty list. A failed /api/boards renders as a failure, with a retry. -->
<QueryState {query} errorTitle="Could not load your boards">
  {#snippet skeleton()}<BoardColumnsSkeleton />{/snippet}
  {#snippet empty()}
    <EmptyState
      icon="⧉"
      title="No boards yet"
      hint="Create one from the sidebar to start assigning work to the fleet."
    />
  {/snippet}
  {#snippet children(_boards)}
    <!-- Non-empty: the redirect above is already in flight — hold the shape. -->
    <BoardColumnsSkeleton />
  {/snippet}
</QueryState>

<script lang="ts">
  import { navigate, route } from '@/router'
  import { isUnder } from '@/lib/route-tabs'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { useBoards } from '@/lib/boards.svelte'
  import BoardSkeleton from './BoardSkeleton.svelte'

  const query = useBoards()
  const firstId = $derived(query.data?.[0]?.id)

  // Open the most recent board automatically (Plane/ClickUp behaviour).
  //
  // Guarded on still being here, because this effect fires on the QUERY
  // resolving rather than on navigation: leave the bare /boards while the list
  // is still in flight and the redirect lands after the click, pulling you back
  // to a board you had just navigated away from. Narrower than the Comms case
  // (that one re-fired on the pathname itself) but the same failure — see
  // `isUnder`.
  $effect(() => {
    if (!isUnder(route.pathname, '/boards')) return
    if (firstId) void navigate('/boards/:boardId', { params: { boardId: firstId }, replace: true })
  })
</script>

<!-- "No boards yet" is now reachable ONLY from a 200 that really carried an
     empty list. A failed /api/boards renders as a failure, with a retry. -->
<QueryState {query} errorTitle="Could not load your boards">
  {#snippet skeleton()}<BoardSkeleton />{/snippet}
  {#snippet empty()}
    <EmptyState
      icon="⧉"
      title="No boards yet"
      hint="Create one from the sidebar to start assigning work to your agents."
    />
  {/snippet}
  {#snippet children(_boards)}
    <!-- Non-empty: the redirect above is already in flight — hold the shape. -->
    <BoardSkeleton />
  {/snippet}
</QueryState>

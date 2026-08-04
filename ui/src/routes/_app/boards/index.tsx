import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { EmptyState } from '@/components/ui/empty-state'
import { QueryState } from '@/components/ui/query-state'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { useBoards } from '@/lib/boards'

export const Route = createFileRoute('/_app/boards/')({
  component: BoardsIndex,
})

// Same column skeleton as the board page, so the redirect lands seamlessly.
function BoardColumnsSkeleton() {
  return (
    <div className="grid h-full grid-cols-2 gap-3 overflow-hidden p-6 sm:grid-cols-4">
      {[0, 1, 2, 3].map((c) => (
        <div key={c} className="space-y-3">
          <Skeleton className="h-3 w-20 rounded-full" delay={c * 0.1} />
          {[0, 1, 2].map((r) => (
            <SkeletonCard key={r} delay={c * 0.1 + r * 0.15} />
          ))}
        </div>
      ))}
    </div>
  )
}

function BoardsIndex() {
  const query = useBoards()
  const navigate = useNavigate()
  const firstId = query.data?.[0]?.id

  // Open the most recent board automatically (Plane/ClickUp behaviour).
  useEffect(() => {
    if (firstId) void navigate({ to: '/boards/$boardId', params: { boardId: firstId }, replace: true })
  }, [firstId, navigate])

  // "No boards yet" is now reachable ONLY from a 200 that really carried an
  // empty list. A failed /api/boards renders as a failure, with a retry.
  return (
    <QueryState
      query={query}
      skeleton={<BoardColumnsSkeleton />}
      errorTitle="Could not load your boards"
      empty={
        <EmptyState
          icon="⧉"
          title="No boards yet"
          hint="Create one from the sidebar to start assigning work to the fleet."
        />
      }
    >
      {/* Non-empty: the redirect above is already in flight — hold the shape. */}
      {() => <BoardColumnsSkeleton />}
    </QueryState>
  )
}

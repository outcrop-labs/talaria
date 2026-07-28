import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { useBoards } from '@/lib/boards'

export const Route = createFileRoute('/_app/boards/')({
  component: BoardsIndex,
})

function BoardsIndex() {
  const { data: boards = [], isLoading } = useBoards()
  const navigate = useNavigate()

  // Open the most recent board automatically (Plane/ClickUp behaviour).
  useEffect(() => {
    if (boards[0]) void navigate({ to: '/boards/$boardId', params: { boardId: boards[0].id }, replace: true })
  }, [boards, navigate])

  if (isLoading || boards[0]) {
    // Same column skeleton as the board page, so the redirect lands seamlessly.
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
  return (
    <EmptyState
      icon="⧉"
      title="No boards yet"
      hint="Create one from the sidebar to start assigning work to the fleet."
    />
  )
}

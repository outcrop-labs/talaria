import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { EmptyState } from '@/components/ui/empty-state'
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
    return <div className="grid h-full place-items-center text-sm text-muted">Loading</div>
  }
  return (
    <EmptyState
      icon="⧉"
      title="No boards yet"
      hint="Create one from the sidebar to start assigning work to the fleet."
    />
  )
}

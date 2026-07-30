import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { TaskDetail } from '@/components/board/task-detail'
import { useBoards, useArchivedBoards } from '@/lib/boards'

// Directly-linkable ticket route, nested under the board so the board stays
// rendered behind the modal. Landing here directly opens the board + ticket.
export const Route = createFileRoute('/_app/boards/$boardId/$taskId')({
  component: TicketRoute,
})

function TicketRoute() {
  const { boardId, taskId } = Route.useParams()
  const navigate = useNavigate()
  const { data: boards = [] } = useBoards()
  const { data: archivedBoards = [] } = useArchivedBoards()
  const board = boards.find((b) => b.id === boardId) ?? archivedBoards.find((b) => b.id === boardId)

  if (!board) return null // board still loading (or no access); parent handles empty state
  return (
    <TaskDetail
      taskId={taskId}
      board={board}
      onClose={() => void navigate({ to: '/boards/$boardId', params: { boardId }, search: (prev: Record<string, unknown>) => prev })}
    />
  )
}

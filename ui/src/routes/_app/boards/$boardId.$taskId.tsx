import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { TaskDetail } from '@/components/board/task-detail'
import { listQuery } from '@/components/ui/query-state'
import { useBoards, useArchivedBoards } from '@/lib/boards'

// Directly-linkable ticket route, nested under the board so the board stays
// rendered behind the modal. Landing here directly opens the board + ticket.
export const Route = createFileRoute('/_app/boards/$boardId/$taskId')({
  component: TicketRoute,
})

function TicketRoute() {
  const { boardId, taskId } = Route.useParams()
  const navigate = useNavigate()
  // This route is what a pasted ticket link lands on. Both lists defaulted to
  // `[]`, so `board` came out undefined and the whole modal returned null: a
  // link that opens a board with no ticket on it and no word about why. The
  // rows behave the same; the failure now has somewhere to go.
  const boardsList = listQuery(useBoards(), { title: 'Could not open this ticket', variant: 'full' })
  const archivedList = listQuery(useArchivedBoards(), { title: 'Could not open this ticket', variant: 'full' })
  const board = boardsList.rows.find((b) => b.id === boardId) ?? archivedList.rows.find((b) => b.id === boardId)

  // Nothing to look the ticket up IN. EITHER read failing is enough: the ticket
  // may live on an archived board, so a live-boards list that came back fine
  // does not license the conclusion "no such board" when the archived read
  // broke. `&&` here meant a failed archived read fell through to the silent
  // `return null` below — the same blank modal this branch exists to replace.
  // The board page behind this modal reports the failure for itself; this says
  // why the ticket did not open.
  if (!board && (boardsList.failed || archivedList.failed))
    return (
      <div className="fixed inset-0 z-40 grid place-items-center bg-black/50 p-4" onClick={() => void navigate({ to: '/boards/$boardId', params: { boardId }, search: (prev: Record<string, unknown>) => prev })}>
        <div className="w-full max-w-sm rounded-2xl border border-line-subtle bg-card p-4" onClick={(e) => e.stopPropagation()}>
          {boardsList.notice ?? archivedList.notice}
        </div>
      </div>
    )
  if (!board) return null // board still loading (or no access); parent handles empty state
  return (
    <TaskDetail
      taskId={taskId}
      board={board}
      onClose={() => void navigate({ to: '/boards/$boardId', params: { boardId }, search: (prev: Record<string, unknown>) => prev })}
    />
  )
}

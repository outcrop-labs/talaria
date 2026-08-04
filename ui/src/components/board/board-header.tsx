import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Settings2, Archive } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { Skeleton } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { renameBoard, useBoardMembers, type Board } from '@/lib/boards'

// Board page header: editable name, stacked member avatars, and a single settings
// gear (everything else lives in the board settings modal).
export function BoardHeader({ board, onSettings }: { board: Board; onSettings: () => void }) {
  const qc = useQueryClient()
  // `{ data: members = [] }` turned a 500 on /api/boards/:id/members into an
  // empty avatar stack — byte-identical to a board only you can see. Who has
  // access is a security-shaped claim; it may only be made from a read that
  // actually returned.
  const membersQuery = useBoardMembers(board.id)
  const members = membersQuery.data ?? []
  const membersLoading = membersQuery.isLoading
  const membersFailed = membersQuery.isError && membersQuery.data === undefined
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const [name, setName] = useState(board.name)
  const [editing, setEditing] = useState(false)

  const commit = async () => {
    setEditing(false)
    const n = name.trim()
    if (!n || n === board.name) return setName(board.name)
    await renameBoard(board.id, n)
    void qc.invalidateQueries({ queryKey: ['boards'] })
  }

  return (
    <div className="flex items-center gap-3 border-b border-line-subtle px-5 py-3">
      {board.archivedAt && (
        <span className="flex items-center gap-1 rounded-md border border-line bg-raised px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
          <Archive size={12} /> Archived
        </span>
      )}
      {editing ? (
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && commit()}
          className="min-w-0 flex-1 border-0 bg-transparent font-sans text-lg font-semibold text-fg outline-none"
        />
      ) : (
        <button
          type="button"
          onClick={() => canEdit && !board.archivedAt && setEditing(true)}
          className="min-w-0 flex-1 truncate text-left font-sans text-lg font-semibold text-fg"
          title={canEdit ? 'Rename' : undefined}
        >
          {board.name}
        </button>
      )}

      {membersFailed && (
        <QueryError
          variant="inline"
          className="shrink-0 text-right"
          title="Members unavailable"
          error={membersQuery.error}
          onRetry={() => void membersQuery.refetch()}
        />
      )}

      <div className="flex -space-x-2">
        {membersLoading &&
          [0, 1].map((i) => (
            <Skeleton key={i} className="h-7 w-7 rounded-full ring-2 ring-[color:var(--theme-panel)]" delay={i * 0.12} />
          ))}
        {members.slice(0, 5).map((m) => (
          <Avatar key={m.userId} name={m.email ?? m.name} className="h-7 w-7 ring-2 ring-[color:var(--theme-panel)]" />
        ))}
        {members.length > 5 && (
          <span className="grid h-7 w-7 place-items-center rounded-full border border-line-strong bg-raised font-mono text-[10px] tracking-[0.05em] text-muted ring-2 ring-[color:var(--theme-panel)]">
            +{members.length - 5}
          </span>
        )}
      </div>

      {canEdit && (
        <button
          type="button"
          onClick={onSettings}
          className="grid h-8 w-8 place-items-center rounded-md text-muted transition-colors hover:bg-hover hover:text-fg"
          title="Board settings"
          aria-label="Board settings"
        >
          <Settings2 size={17} />
        </button>
      )}
    </div>
  )
}

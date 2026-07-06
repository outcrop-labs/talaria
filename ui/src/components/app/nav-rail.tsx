import { useState } from 'react'
import { Link, useRouterState } from '@tanstack/react-router'
import { Plus, Users, Archive } from 'lucide-react'
import { WingMark } from '@/components/brand'
import { TeamsModal } from '@/components/board/teams-modal'
import { CreateBoardModal } from '@/components/board/create-board-modal'
import { cn } from '@/lib/cn'
import { NAV } from '@/lib/nav'
import { useBoards, useArchivedBoards, type Board } from '@/lib/boards'
import { useNotifications } from '@/lib/notifications'
import { useDeniedViews, type SessionUser } from '@/lib/session'

// The main application menu. The Boards item expands to the user's boards
// (grouped by team) when that section is active.
export function NavRail({ user }: { user: SessionUser }) {
  const isAdmin = user.role === 'admin'
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const denied = useDeniedViews()
  const [creating, setCreating] = useState(false)
  const [teamsOpen, setTeamsOpen] = useState(false)
  const unread = useNotifications().data?.unread ?? 0

  return (
    <nav className="flex h-full w-56 shrink-0 flex-col gap-6 overflow-y-auto border-r border-line-subtle bg-sidebar px-3 py-5">
      {NAV.map((section) => {
        const items = section.items.filter((i) => (!i.adminOnly || isAdmin) && !denied.includes(i.to))
        if (items.length === 0) return null
        return (
          <div key={section.title}>
            <div className="px-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted">{section.title}</div>
            <ul className="space-y-0.5">
              {items.map((item) => (
                <li key={item.to}>
                  <Link
                    to={item.to}
                    activeOptions={{ exact: item.to === '/' }}
                    className="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
                    activeProps={{ className: 'flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm bg-card text-fg [&_.nav-ico]:text-accent' }}
                  >
                    <span className="nav-ico w-4 text-center text-muted">{item.icon}</span>
                    <span className="flex-1 truncate">{item.label}</span>
                    {item.to === '/inbox' && unread > 0 && (
                      <span className="rounded-full bg-accent px-1.5 text-[10px] font-semibold text-surface">{unread}</span>
                    )}
                    {item.adminOnly && <span className="text-[10px] text-accent">admin</span>}
                  </Link>
                  {item.to === '/boards' && pathname.startsWith('/boards') && (
                    <BoardsSublist activePath={pathname} onNew={() => setCreating(true)} onTeams={() => setTeamsOpen(true)} />
                  )}
                </li>
              ))}
            </ul>
          </div>
        )
      })}

      <div className="mt-auto flex items-center gap-2 px-2 pt-2 text-[10px] text-muted">
        <WingMark className="h-4 w-4" />
        <span>Talaria · Phase 2</span>
      </div>

      <CreateBoardModal open={creating} onClose={() => setCreating(false)} />
      <TeamsModal open={teamsOpen} onClose={() => setTeamsOpen(false)} />
    </nav>
  )
}

function BoardsSublist({ activePath, onNew, onTeams }: { activePath: string; onNew: () => void; onTeams: () => void }) {
  const { data: boards = [] } = useBoards()
  const { data: archived = [] } = useArchivedBoards()
  const [showArchived, setShowArchived] = useState(false)

  const groups = new Map<string, Board[]>()
  for (const b of boards) {
    const key = b.teamName ?? 'Personal'
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key)!.push(b)
  }
  const ordered = [...groups.entries()].sort((a, b) => (a[0] === 'Personal' ? -1 : b[0] === 'Personal' ? 1 : a[0].localeCompare(b[0])))

  const boardLink = (b: Board) => (
    <Link
      to="/boards/$boardId"
      params={{ boardId: b.id }}
      className={cn(
        'block truncate rounded-md px-2 py-1 text-xs transition-colors hover:bg-card hover:text-fg',
        activePath === `/boards/${b.id}` ? 'bg-card text-fg' : 'text-muted',
      )}
    >
      {b.name}
    </Link>
  )

  return (
    <div className="ml-3 mt-0.5 space-y-2 border-l border-line-subtle pl-2">
      {ordered.map(([group, gboards]) => (
        <div key={group}>
          <div className="px-2 text-[9px] font-semibold uppercase tracking-wider text-muted opacity-70">{group}</div>
          <ul className="space-y-0.5">
            {gboards.map((b) => (
              <li key={b.id}>{boardLink(b)}</li>
            ))}
          </ul>
        </div>
      ))}

      {archived.length > 0 && (
        <div>
          <button
            onClick={() => setShowArchived((v) => !v)}
            className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-[9px] font-semibold uppercase tracking-wider text-muted opacity-70 transition-colors hover:text-fg"
          >
            <Archive size={11} /> Archived ({archived.length}) <span className="ml-auto">{showArchived ? '▾' : '▸'}</span>
          </button>
          {showArchived && (
            <ul className="space-y-0.5">
              {archived.map((b) => (
                <li key={b.id}>{boardLink(b)}</li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="flex flex-col gap-0.5">
        <button onClick={onNew} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors hover:text-accent">
          <Plus size={13} /> New board
        </button>
        <button onClick={onTeams} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors hover:text-accent">
          <Users size={13} /> Manage teams
        </button>
      </div>
    </div>
  )
}

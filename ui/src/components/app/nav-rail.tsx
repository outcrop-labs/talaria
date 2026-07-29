import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Plus, Users, Archive, ExternalLink, Link as LinkIcon } from 'lucide-react'
import { useContextMenu, copyAppLink } from '@/components/ui/context-menu'
import { TeamsModal } from '@/components/board/teams-modal'
import { CreateBoardModal } from '@/components/board/create-board-modal'
import { alert } from '@/components/ui/confirm'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import { NAV, type NavItem } from '@/lib/nav'
import { useBoards, useArchivedBoards, moveBoardToTeam, type Board } from '@/lib/boards'
import { useTeams } from '@/lib/teams'
import { useNotifications } from '@/lib/notifications'
import { useDeniedViews, useHasPerm, type SessionUser } from '@/lib/session'
import { useEnabledApps } from '@/lib/apps'

// The main application menu. The Boards item expands to the user's boards
// (grouped by team) when that section is active.
export function NavRail({ user }: { user: SessionUser }) {
  const isAdmin = user.role === 'admin'
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const denied = useDeniedViews()
  const [creating, setCreating] = useState(false)
  const [teamsOpen, setTeamsOpen] = useState(false)
  const unread = useNotifications().data?.unread ?? 0
  // Enabled apps slot into the sections as if they shipped with the platform:
  // work surfaces under Work, manage surfaces under Manage (grant-gated like
  // any core Manage view via deniedViews).
  const { data: apps = [] } = useEnabledApps()
  const appItems: Record<string, NavItem[]> = {
    Work: apps.filter((a) => a.surfaces.work).map((a) => ({ to: `/x/${a.slug}`, label: a.surfaces.work!, icon: a.icon })),
    Manage: apps.filter((a) => a.surfaces.manage).map((a) => ({ to: `/x/${a.slug}/manage`, label: a.surfaces.manage!, icon: a.icon })),
  }

  return (
    <nav className="flex h-full w-56 shrink-0 flex-col gap-6 overflow-y-auto border-r border-line-subtle bg-sidebar px-3 py-5">
      {NAV.map((section) => {
        if (section.adminOnly && !isAdmin) return null
        const items = [...section.items, ...(appItems[section.title] ?? [])].filter(
          (i) => (!i.adminOnly || isAdmin) && !denied.includes(i.to) && !denied.some((d) => i.to.startsWith(d + '/')),
        )
        if (items.length === 0) return null
        return (
          <div key={section.title}>
            <div className="px-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted">{section.title}</div>
            <ul className="space-y-0.5">
              {items.map((item) => (
                <li key={item.to}>
                  <Link
                    to={item.to}
                    // Exact for Home and for app WORK items: /x/<slug> is a
                    // path prefix of its sibling /x/<slug>/manage, and fuzzy
                    // matching would light both up at once.
                    activeOptions={{ exact: item.to === '/' || appItems.Manage!.some((m) => m.to.startsWith(item.to + '/')) }}
                    className="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
                    activeProps={{ className: 'flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm bg-card text-fg [&_.nav-ico]:text-accent' }}
                  >
                    <span className="nav-ico w-4 text-center text-muted">{item.icon}</span>
                    <span className="flex-1 truncate">{item.label}</span>
                    {item.to === '/' && unread > 0 && (
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

      <CreateBoardModal open={creating} onClose={() => setCreating(false)} />
      <TeamsModal open={teamsOpen} onClose={() => setTeamsOpen(false)} />
    </nav>
  )
}

function BoardsSublist({ activePath, onNew, onTeams }: { activePath: string; onNew: () => void; onTeams: () => void }) {
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { openMenu, menu } = useContextMenu()
  const { data: boards = [], isLoading: boardsLoading } = useBoards()
  const { data: archived = [] } = useArchivedBoards()
  const { data: teams = [], isLoading: teamsLoading } = useTeams()
  const [showArchived, setShowArchived] = useState(false)
  // Drag a board you own onto a team header to move it (Personal = no team).
  const [dragging, setDragging] = useState<Board | null>(null)
  const [overGroup, setOverGroup] = useState<string | null>(null)

  const groups = new Map<string, { teamId: string | null; boards: Board[] }>()
  groups.set('Personal', { teamId: null, boards: [] })
  for (const t of teams) groups.set(t.name, { teamId: t.id, boards: [] })
  for (const b of boards) {
    const key = b.teamName ?? 'Personal'
    if (!groups.has(key)) groups.set(key, { teamId: b.teamId, boards: [] })
    groups.get(key)!.boards.push(b)
  }
  const ordered = [...groups.entries()]
    .filter(([key, g]) => g.boards.length > 0 || (dragging !== null && key !== (dragging.teamName ?? 'Personal')))
    .sort((a, b) => (a[0] === 'Personal' ? -1 : b[0] === 'Personal' ? 1 : a[0].localeCompare(b[0])))

  const drop = async (teamId: string | null, key: string) => {
    setOverGroup(null)
    const b = dragging
    setDragging(null)
    if (!b || (b.teamId ?? null) === teamId) return
    const r = (await moveBoardToTeam(b.id, teamId)) as { error?: string }
    if (r?.error) void alert({ title: 'Could not move board', message: r.error })
    await qc.invalidateQueries({ queryKey: ['boards'] })
    void key
  }

  const boardLink = (b: Board) => (
    <Link
      to="/boards/$boardId"
      params={{ boardId: b.id }}
      draggable={b.role === 'owner'}
      onContextMenu={(e) =>
        openMenu(e, [
          { label: 'Open', icon: <ExternalLink size={14} />, onSelect: () => void navigate({ to: '/boards/$boardId', params: { boardId: b.id } }) },
          { label: 'Copy link', icon: <LinkIcon size={14} />, onSelect: () => copyAppLink(`/boards/${b.id}`) },
        ])
      }
      onDragStart={() => setDragging(b)}
      onDragEnd={() => {
        setDragging(null)
        setOverGroup(null)
      }}
      className={cn(
        'block truncate rounded-md px-2 py-1 text-xs transition-colors hover:bg-card hover:text-fg',
        activePath === `/boards/${b.id}` ? 'bg-card text-fg' : 'text-muted',
        dragging?.id === b.id && 'opacity-40',
      )}
    >
      {b.name}
    </Link>
  )

  // While boards/teams load, hold the sublist's shape (group headers + rows) so
  // the footer buttons don't render alone and get pushed down when data lands.
  if (boardsLoading || teamsLoading)
    return (
      <div className="ml-3 mt-0.5 space-y-2 border-l border-line-subtle pl-2">
        <div className="space-y-1.5 px-2 py-0.5">
          <Skeleton className="h-2 w-12 rounded-full" />
          <Skeleton className="h-2.5 w-24 rounded-full" delay={0.12} />
          <Skeleton className="h-2.5 w-20 rounded-full" delay={0.24} />
        </div>
        <div className="space-y-1.5 px-2 py-0.5">
          <Skeleton className="h-2 w-14 rounded-full" delay={0.36} />
          <Skeleton className="h-2.5 w-24 rounded-full" delay={0.48} />
        </div>
        <SublistFooter onNew={onNew} onTeams={onTeams} />
      </div>
    )

  return (
    <div className="ml-3 mt-0.5 space-y-2 border-l border-line-subtle pl-2">
      {ordered.map(([group, g]) => (
        <div
          key={group}
          onDragOver={(e) => {
            if (!dragging) return
            e.preventDefault()
            setOverGroup(group)
          }}
          onDragLeave={() => overGroup === group && setOverGroup(null)}
          onDrop={(e) => {
            e.preventDefault()
            void drop(g.teamId, group)
          }}
          className={cn(dragging && overGroup === group && 'rounded-md bg-card ring-1 ring-[color:var(--theme-accent-border)]')}
        >
          <div className="px-2 text-[9px] font-semibold uppercase tracking-wider text-muted opacity-70">{group}</div>
          <ul className="space-y-0.5">
            {g.boards.map((b) => (
              <li key={b.id}>{boardLink(b)}</li>
            ))}
            {g.boards.length === 0 && dragging && (
              <li className="px-2 py-0.5 text-[10px] italic text-muted">drop here</li>
            )}
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

      <SublistFooter onNew={onNew} onTeams={onTeams} />
      {menu}
    </div>
  )
}

function SublistFooter({ onNew, onTeams }: { onNew: () => void; onTeams: () => void }) {
  const mayCreate = useHasPerm('boards.create')
  return (
    <div className="flex flex-col gap-0.5">
      {mayCreate && (
        <button onClick={onNew} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors hover:text-accent">
          <Plus size={13} /> New board
        </button>
      )}
      <button onClick={onTeams} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors hover:text-accent">
        <Users size={13} /> Manage teams
      </button>
    </div>
  )
}

import { useEffect, useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import { Plus, Users, Archive, ChevronRight, ExternalLink, Link as LinkIcon } from 'lucide-react'
import { useContextMenu, copyAppLink } from '@/components/ui/context-menu'
import { TeamsModal } from '@/components/board/teams-modal'
import { CreateBoardModal } from '@/components/board/create-board-modal'
import { alert } from '@/components/ui/confirm'
import { Skeleton } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
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

// Which groups the user has expanded, keyed by group name. Persisted so the
// rail comes back the way it was left; unknown groups fall back to their
// default (teams open, Archived closed) rather than being stored eagerly.
const GROUPS_KEY = 'talaria:board-groups'
const ARCHIVED_KEY = ' archived'

function loadGroupState(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(GROUPS_KEY)
    if (raw) {
      const v = JSON.parse(raw) as unknown
      if (v && typeof v === 'object' && !Array.isArray(v)) {
        return Object.fromEntries(Object.entries(v as Record<string, unknown>).filter(([, o]) => typeof o === 'boolean')) as Record<string, boolean>
      }
    }
  } catch {
    /* ignore */
  }
  return {}
}

const byName = (a: Board, b: Board) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' })

function BoardsSublist({ activePath, onNew, onTeams }: { activePath: string; onNew: () => void; onTeams: () => void }) {
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { openMenu, menu } = useContextMenu()
  const boardsQuery = useBoards()
  const archivedQuery = useArchivedBoards()
  const teamsQuery = useTeams()
  const { data: boards = [], isLoading: boardsLoading } = boardsQuery
  const { data: archived = [] } = archivedQuery
  const { data: teams = [], isLoading: teamsLoading } = teamsQuery
  // Init to defaults (SSR-safe), then hydrate from localStorage on the client.
  const [groupState, setGroupState] = useState<Record<string, boolean>>({})
  useEffect(() => setGroupState(loadGroupState()), [])
  // Drag a board you own onto a team header to move it (Personal = no team).
  const [dragging, setDragging] = useState<Board | null>(null)
  const [overGroup, setOverGroup] = useState<string | null>(null)

  const toggleGroup = (key: string, isOpen: boolean) =>
    setGroupState((s) => {
      const next = { ...s, [key]: !isOpen }
      try {
        localStorage.setItem(GROUPS_KEY, JSON.stringify(next))
      } catch {
        /* ignore */
      }
      return next
    })

  const groups = new Map<string, { teamId: string | null; boards: Board[] }>()
  groups.set('Personal', { teamId: null, boards: [] })
  for (const t of teams) groups.set(t.name, { teamId: t.id, boards: [] })
  for (const b of boards) {
    const key = b.teamName ?? 'Personal'
    if (!groups.has(key)) groups.set(key, { teamId: b.teamId, boards: [] })
    groups.get(key)!.boards.push(b)
  }
  for (const g of groups.values()) g.boards.sort(byName)
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
        // Indented past the group chevron so the nesting reads at a glance.
        'block truncate rounded-md py-1 pl-[22px] pr-2 text-xs transition-colors hover:bg-card hover:text-fg',
        activePath === `/boards/${b.id}` ? 'bg-card font-medium text-fg' : 'text-muted',
        dragging?.id === b.id && 'opacity-40',
      )}
    >
      {b.name}
    </Link>
  )

  const retryLists = () => {
    void boardsQuery.refetch()
    void teamsQuery.refetch()
  }
  // Either read failing makes the rail's grouping suspect (teams supply the
  // group names), so one marker covers both — named for whichever broke.
  const staleFailure = boardsQuery.isError || teamsQuery.isError

  // While boards/teams load, hold the sublist's shape (group headers + rows) so
  // the footer buttons don't render alone and get pushed down when data lands.
  if (boardsLoading || teamsLoading)
    return (
      <div className="ml-3 mt-1 border-l border-line-subtle pl-2">
        <div className="space-y-1.5 px-1.5 py-1">
          <Skeleton className="h-2.5 w-16 rounded-full" />
          <Skeleton className="ml-3 h-2.5 w-24 rounded-full" delay={0.12} />
          <Skeleton className="ml-3 h-2.5 w-20 rounded-full" delay={0.24} />
          <Skeleton className="h-2.5 w-14 rounded-full" delay={0.36} />
          <Skeleton className="ml-3 h-2.5 w-24 rounded-full" delay={0.48} />
        </div>
        <SublistFooter onNew={onNew} onTeams={onTeams} />
      </div>
    )

  // A failed read is not "you have no boards". `data` is undefined on error and
  // the `= []` default turns that into an empty list, so every group filters
  // out and the rail renders a header, a footer, and nothing between them — the
  // original incident, still alive in the sidebar. Say it broke; offer a retry.
  //
  // But only when the failure left NOTHING to show. React Query keeps the last
  // good list through a failed background refetch, and blanking a populated rail
  // over a transient blip — boards vanishing mid-session — is the worse lie of
  // the two. Stale beats blank: below, the list still renders and the failure
  // rides along as an inline marker. (Same rule QueryState applies.)
  if (boardsQuery.isError && boardsQuery.data === undefined)
    return (
      <div className="ml-3 mt-1 border-l border-line-subtle pl-2">
        <QueryError
          variant="inline"
          className="px-1.5 py-1"
          title="Could not load boards"
          error={boardsQuery.error}
          onRetry={retryLists}
        />
        <SublistFooter onNew={onNew} onTeams={onTeams} />
      </div>
    )

  return (
    <div className="ml-3 mt-1 space-y-1 border-l border-line-subtle pl-2">
      {/* The list below is the last good read — a refresh failed. Keeping the
          boards on screen and marking them stale beats replacing them with an
          error, but saying nothing would let the rail pass off old data as
          current. */}
      {staleFailure && (
        <QueryError
          variant="inline"
          className="px-1.5 py-1"
          title={boardsQuery.isError ? 'Boards may be out of date' : 'Teams may be out of date'}
          error={boardsQuery.isError ? boardsQuery.error : teamsQuery.error}
          onRetry={retryLists}
        />
      )}
      {ordered.map(([group, g]) => {
        // Groups start open; a drag force-opens every one so there is always a
        // visible drop target. A collapsed group still shows the board you are
        // currently on, so the rail never hides where you are.
        const open = (groupState[group] ?? true) || dragging !== null
        const rows = open ? g.boards : g.boards.filter((b) => activePath === `/boards/${b.id}`)
        return (
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
            <GroupHeader label={group} count={g.boards.length} open={open} onToggle={() => toggleGroup(group, open)} />
            <ul className="space-y-px">
              {rows.map((b) => (
                <li key={b.id}>{boardLink(b)}</li>
              ))}
              {g.boards.length === 0 && dragging && <li className="py-0.5 pl-[22px] text-[10px] italic text-muted">drop here</li>}
            </ul>
          </div>
        )
      })}

      {/* Archived fails on its own budget: the live boards above loaded, so
          they stay. Omitting this section silently would read as "nothing is
          archived", which is a different claim from "we couldn't ask". */}
      {archivedQuery.isError && (
        <QueryError
          variant="inline"
          className="px-1.5 py-1"
          title="Could not load archived"
          error={archivedQuery.error}
          onRetry={() => void archivedQuery.refetch()}
        />
      )}

      {archived.length > 0 &&
        (() => {
          const open = groupState[ARCHIVED_KEY] ?? false
          return (
            <div>
              <GroupHeader
                label="Archived"
                icon={<Archive size={11} />}
                count={archived.length}
                open={open}
                onToggle={() => toggleGroup(ARCHIVED_KEY, open)}
              />
              {open && (
                <ul className="space-y-px">
                  {[...archived].sort(byName).map((b) => (
                    <li key={b.id}>{boardLink(b)}</li>
                  ))}
                </ul>
              )}
            </div>
          )
        })()}

      <SublistFooter onNew={onNew} onTeams={onTeams} />
      {menu}
    </div>
  )
}

// A collapsible group label. Title-case at 11px rather than the rail's uppercase
// micro-label — that treatment is reserved for the top-level sections, and long
// team names ("Marketing + Tech Collab") are unreadable in wide-tracked caps.
function GroupHeader({
  label,
  icon,
  count,
  open,
  onToggle,
}: {
  label: string
  icon?: ReactNode
  count: number
  open: boolean
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      className="flex w-full items-center gap-1 rounded-md py-1 pl-1 pr-2 text-left text-[11px] text-muted transition-colors hover:bg-card hover:text-fg"
    >
      <ChevronRight size={11} className={cn('shrink-0 transition-transform duration-150', open && 'rotate-90')} />
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span className="shrink-0 text-[10px] tabular-nums opacity-50">{count}</span>
    </button>
  )
}

function SublistFooter({ onNew, onTeams }: { onNew: () => void; onTeams: () => void }) {
  const mayCreate = useHasPerm('boards.create')
  return (
    // Divided off from the list so the actions don't read as another board row.
    <div className="mt-1.5 flex flex-col gap-px border-t border-line-subtle pt-1.5">
      {mayCreate && (
        <button onClick={onNew} className="flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs text-muted transition-colors hover:bg-card hover:text-accent">
          <Plus size={13} className="shrink-0" /> New board
        </button>
      )}
      <button onClick={onTeams} className="flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs text-muted transition-colors hover:bg-card hover:text-accent">
        <Users size={13} className="shrink-0" /> Manage teams
      </button>
    </div>
  )
}

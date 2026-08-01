import { Fragment, useCallback, useState, useSyncExternalStore, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouterState } from '@tanstack/react-router'
import {
  Archive,
  ChevronsLeft,
  ChevronsRight,
  ExternalLink,
  Link as LinkIcon,
  Plus,
  Users,
} from 'lucide-react'
import { useContextMenu, copyAppLink } from '@/components/ui/context-menu'
import { TeamsModal } from '@/components/board/teams-modal'
import { CreateBoardModal } from '@/components/board/create-board-modal'
import { alert } from '@/components/ui/confirm'
import { Skeleton } from '@/components/ui/skeleton'
import { Brand, WingMark } from '@/components/brand'
import { cn } from '@/lib/cn'
import { NAV, type NavItem } from '@/lib/nav'
import { useBoards, useArchivedBoards, moveBoardToTeam, type Board } from '@/lib/boards'
import { useTeams } from '@/lib/teams'
import { useNotifications } from '@/lib/notifications'
import { useDeniedViews, useHasPerm, type SessionUser } from '@/lib/session'
import { useEnabledApps } from '@/lib/apps'

// Gentle dew shell (spec §5): one collapsible nav — 208px sidebar ⇄ 64px icon
// rail. The choice persists in localStorage, but the server can't see
// localStorage — reading it during the first render made SSR (expanded) and
// client (collapsed) disagree and hydration fail. useSyncExternalStore is the
// hydration-safe channel: the server snapshot (expanded) is what hydration
// matches against, then React swaps in the persisted client snapshot in the
// immediate follow-up render. NavRail and the _app.tsx loading skeleton share
// the same store so both frames always agree.
const NAV_COLLAPSED_KEY = 'talaria:nav-collapsed'
const NAV_COLLAPSED_EVENT = 'talaria:nav-collapsed'

// In-memory fallback so the toggle still works when localStorage is
// unavailable (private mode) — the choice simply won't persist.
let collapsedFallback = false

function readNavCollapsed(): boolean {
  try {
    return window.localStorage.getItem(NAV_COLLAPSED_KEY) === '1'
  } catch {
    return collapsedFallback
  }
}

function subscribeNavCollapsed(onChange: () => void): () => void {
  window.addEventListener(NAV_COLLAPSED_EVENT, onChange)
  window.addEventListener('storage', onChange) // sync across tabs
  return () => {
    window.removeEventListener(NAV_COLLAPSED_EVENT, onChange)
    window.removeEventListener('storage', onChange)
  }
}

export function useNavCollapsed(): { collapsed: boolean; toggleCollapsed: () => void } {
  const collapsed = useSyncExternalStore(subscribeNavCollapsed, readNavCollapsed, () => false)
  const toggleCollapsed = useCallback(() => {
    const next = !readNavCollapsed()
    collapsedFallback = next
    try {
      window.localStorage.setItem(NAV_COLLAPSED_KEY, next ? '1' : '0')
    } catch {
      /* private mode — state simply won't persist */
    }
    window.dispatchEvent(new Event(NAV_COLLAPSED_EVENT))
  }, [])
  return { collapsed, toggleCollapsed }
}

// The main application menu. Expanded: WORK/MANAGE/SYSTEM sections with the
// Boards sublist; collapsed: 36px icon tiles with tooltips. Active state uses
// TanStack Router's data-status attribute so the Link's activeOptions remain
// the single source of truth for matching.
export function NavRail({ user }: { user: SessionUser }) {
  const isAdmin = user.role === 'admin'
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const denied = useDeniedViews()
  const [creating, setCreating] = useState(false)
  const [teamsOpen, setTeamsOpen] = useState(false)
  const { collapsed, toggleCollapsed } = useNavCollapsed()
  const unread = useNotifications().data?.unread ?? 0
  // Enabled apps slot into the sections as if they shipped with the platform:
  // work surfaces under Work, manage surfaces under Manage (grant-gated like
  // any core Manage view via deniedViews).
  const { data: apps = [] } = useEnabledApps()
  const appItems: Record<string, NavItem[]> = {
    Work: apps.filter((a) => a.surfaces.work).map((a) => ({ to: `/x/${a.slug}`, label: a.surfaces.work!, icon: a.icon })),
    Manage: apps.filter((a) => a.surfaces.manage).map((a) => ({ to: `/x/${a.slug}/manage`, label: a.surfaces.manage!, icon: a.icon })),
  }

  // Denied-view + role filtering, shared by both modes.
  const sections = NAV.flatMap((section) => {
    if (section.adminOnly && !isAdmin) return []
    const items = [...section.items, ...(appItems[section.title] ?? [])].filter(
      (i) => (!i.adminOnly || isAdmin) && !denied.includes(i.to) && !denied.some((d) => i.to.startsWith(d + '/')),
    )
    return items.length === 0 ? [] : [{ title: section.title, items }]
  })

  // Exact for Home and for app WORK items: /x/<slug> is a path prefix of its
  // sibling /x/<slug>/manage, and fuzzy matching would light both up at once.
  const exactFor = (item: NavItem) => item.to === '/' || appItems.Manage!.some((m) => m.to.startsWith(item.to + '/'))

  const modals = (
    <>
      <CreateBoardModal open={creating} onClose={() => setCreating(false)} />
      <TeamsModal open={teamsOpen} onClose={() => setTeamsOpen(false)} />
    </>
  )

  if (collapsed) {
    // ── Icon rail (64px, spec §5) ─────────────────────────────────────────
    return (
      <nav className="flex h-full w-16 shrink-0 flex-col items-center border-r border-line bg-sidebar pb-5 pt-3">
        <div className="grid h-9 w-9 shrink-0 place-items-center" aria-label="Talaria">
          <WingMark className="h-5 w-5" />
        </div>

        <div className="mt-3 flex min-h-0 w-full flex-1 flex-col items-center gap-1 overflow-y-auto">
          {sections.map((section, si) => (
            <Fragment key={section.title}>
              {si > 0 && <div className="my-2 h-px w-6 shrink-0 bg-line" />}
              {section.items.map((item) => (
                <RailTooltip key={item.to} label={item.label}>
                  <Link
                    to={item.to}
                    activeOptions={{ exact: exactFor(item) }}
                    aria-label={item.label}
                    className={cn(
                      'relative grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
                      'data-[status=active]:bg-raised data-[status=active]:text-fg',
                      '[&[data-status=active]_svg]:h-[22px] [&[data-status=active]_svg]:w-[22px]',
                    )}
                  >
                    <NavIcon icon={item.icon} />
                    {item.to === '/' && unread > 0 && (
                      <span className="absolute right-0.5 top-0 font-mono text-[10px] leading-3 tracking-[0.05em] text-muted">
                        {unread}
                      </span>
                    )}
                  </Link>
                </RailTooltip>
              ))}
            </Fragment>
          ))}
        </div>

        <div className="mt-3 flex shrink-0 flex-col items-center gap-3">
          <RailTooltip label="Expand">
            <button
              type="button"
              onClick={toggleCollapsed}
              aria-label="Expand navigation"
              className="grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg"
            >
              <ChevronsRight size={16} strokeWidth={1.5} />
            </button>
          </RailTooltip>
          <RailTooltip label={user.name ?? user.email ?? 'Account'}>
            <div className="grid h-[26px] w-[26px] place-items-center rounded-full border border-line-strong bg-raised font-mono text-[10px] font-medium tracking-[0.05em] text-muted">
              {userInitials(user)}
            </div>
          </RailTooltip>
        </div>

        {modals}
      </nav>
    )
  }

  // ── Sidebar (208px, spec §5) ────────────────────────────────────────────
  return (
    <nav className="flex h-full w-[208px] shrink-0 flex-col border-r border-line bg-sidebar px-3 pb-4 pt-5">
      <div className="flex h-6 shrink-0 items-center">
        <Brand />
      </div>

      <div className="mt-5 flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto">
        {sections.map((section) => (
          <div key={section.title}>
            <div className="flex h-6 items-center px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              {section.title}
            </div>
            <ul className="space-y-px">
              {section.items.map((item) => (
                <li key={item.to}>
                  <Link
                    to={item.to}
                    activeOptions={{ exact: exactFor(item) }}
                    className={cn(
                      'flex h-[30px] items-center gap-[9px] rounded-md px-2 font-sans text-[13px] leading-4 text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
                      'data-[status=active]:bg-raised data-[status=active]:font-medium data-[status=active]:text-fg',
                      '[&[data-status=active]_.nav-bar]:bg-accent [&[data-status=active]_.nav-ico]:text-fg',
                    )}
                  >
                    <span className="nav-bar h-3.5 w-[3px] shrink-0 rounded-[2px] bg-transparent" aria-hidden />
                    <span className="nav-ico grid h-4 w-4 shrink-0 place-items-center text-muted">
                      <NavIcon icon={item.icon} />
                    </span>
                    <span className="flex-1 truncate">{item.label}</span>
                    {item.to === '/' && unread > 0 && (
                      // Spec §5: nav counts are muted (#8E877E) — not accent.
                      <span className="font-mono text-[10px] leading-3 tracking-[0.05em] text-muted">{unread}</span>
                    )}
                  </Link>
                  {item.to === '/boards' && pathname.startsWith('/boards') && (
                    <BoardsSublist activePath={pathname} onNew={() => setCreating(true)} onTeams={() => setTeamsOpen(true)} />
                  )}
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>

      <div className="mt-4 flex h-5 shrink-0 items-center gap-2 px-1">
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" aria-hidden />
        <span className="truncate font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Local · Operator</span>
        <button
          type="button"
          onClick={toggleCollapsed}
          aria-label="Collapse navigation"
          className="ml-auto grid h-5 w-5 shrink-0 place-items-center rounded text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg"
        >
          <ChevronsLeft size={13} strokeWidth={1.5} />
        </button>
      </div>

      {modals}
    </nav>
  )
}

// Core items carry lucide components; enabled apps inject string glyphs from
// their manifest — both render at the 16px icon slot.
function NavIcon({ icon }: { icon: NavItem['icon'] }) {
  if (typeof icon === 'string')
    return <span className="grid h-4 w-4 place-items-center text-[13px] leading-none">{icon}</span>
  const Icon = icon
  return <Icon size={16} strokeWidth={1.5} />
}

function userInitials(user: SessionUser): string {
  const base = user.name?.trim() || user.email || '?'
  const words = base.split(/\s+/).filter(Boolean)
  const init = words.length >= 2 ? `${words[0]!.charAt(0)}${words[1]!.charAt(0)}` : base.slice(0, 2)
  return init.toUpperCase()
}

// Rail tooltips render into a portal at a fixed position so the rail's
// scroll container can't clip them.
function RailTooltip({ label, children }: { label: string; children: ReactNode }) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null)
  return (
    <div
      onMouseEnter={(e) => {
        const r = e.currentTarget.getBoundingClientRect()
        setPos({ x: r.right + 8, y: r.top + r.height / 2 })
      }}
      onMouseLeave={() => setPos(null)}
      onClick={() => setPos(null)}
    >
      {children}
      {pos &&
        createPortal(
          <div
            style={{ left: pos.x, top: pos.y }}
            className="pointer-events-none fixed z-50 -translate-y-1/2 whitespace-nowrap rounded-md border border-line bg-raised px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-fg"
          >
            {label}
          </div>,
          document.body,
        )}
    </div>
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
        'block truncate rounded-md px-2 py-1 text-xs transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
        activePath === `/boards/${b.id}` ? 'bg-raised text-fg' : 'text-muted',
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
          className={cn(dragging && overGroup === group && 'rounded-md bg-raised ring-1 ring-[color:var(--theme-accent-border)]')}
        >
          <div className="px-2 font-mono text-[9px] uppercase tracking-[0.08em] text-ink-dim">{group}</div>
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
            className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 font-mono text-[9px] uppercase tracking-[0.08em] text-ink-dim transition-colors duration-[120ms] hover:text-fg"
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
        <button onClick={onNew} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors duration-[120ms] hover:text-accent">
          <Plus size={13} /> New board
        </button>
      )}
      <button onClick={onTeams} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors duration-[120ms] hover:text-accent">
        <Users size={13} /> Manage teams
      </button>
    </div>
  )
}

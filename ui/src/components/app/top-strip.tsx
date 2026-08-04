import { useEffect, useRef, useState } from 'react'
import { useRouterState } from '@tanstack/react-router'
import { LogOut, SunMoon } from 'lucide-react'
import { ThemeToggle } from '@/components/theme-toggle'
import { Avatar } from '@/components/ui/avatar'
import { NAV } from '@/lib/nav'
import { useEnabledApps, type AppManifest } from '@/lib/apps'
import type { SessionUser } from '@/lib/session'

// Gentle dew top strip (spec §6): breadcrumb readout, compact search
// affordance, and the account chip. The brand lives in the sidebar now —
// this strip is pure chrome on the ground surface.
export function TopStrip({ user, onLogout }: { user: SessionUser; onLogout: () => void }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const { data: apps = [] } = useEnabledApps()

  return (
    // z-40: the strip (and the user-menu flyover inside it) stacks above all
    // page content and its popovers; modals portal to body at z-50.
    <header className="relative z-40 flex h-12 shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4">
      {/* Spec §6: the whole breadcrumb (including <VIEW>) is 10px mono
          uppercase 0.08em muted — no emphasized segment. */}
      <div className="min-w-0 truncate font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
        <span>Local Command Surface</span>
        <span className="mx-1.5 text-ink-dim">/</span>
        <span>{viewName(pathname, apps)}</span>
      </div>

      <div className="flex shrink-0 items-center gap-3">
        {/* Spec §6 shows a `⌕ SEARCH ⌘K` tile, but Talaria has no global
            search / command palette yet — rendering the tile (or a ⌘K hint)
            would be a fabricated affordance (spec §7: no fabricated data).
            Add it here when a real search ships. */}
        <UserMenu user={user} onLogout={onLogout} />
      </div>
    </header>
  )
}

// Derive the active view's name from the route: nav items first (covers WORK/
// MANAGE/SYSTEM and their subpaths), then enabled-app surfaces, then a plain
// capitalization of the first path segment (e.g. /chat → Chat).
function viewName(pathname: string, apps: AppManifest[]): string {
  if (pathname === '/') return 'Inbox'
  const appMatch = /^\/x\/([^/]+)/.exec(pathname)
  if (appMatch) {
    const app = apps.find((a) => a.slug === appMatch[1])
    if (app) return (pathname.startsWith(`/x/${app.slug}/manage`) ? app.surfaces.manage : app.surfaces.work) ?? app.name
    return appMatch[1]!
  }
  const item = NAV.flatMap((s) => s.items).find(
    (i) => i.to !== '/' && (pathname === i.to || pathname.startsWith(i.to + '/')),
  )
  if (item) return item.label
  const seg = pathname.split('/').filter(Boolean)[0] ?? ''
  return seg ? seg.charAt(0).toUpperCase() + seg.slice(1) : 'Inbox'
}

// The strip's only control: status dot + account email, with everything
// personal nested in a flyover (profile, theme, sign out). Settings and Admin
// moved to the sidebar SYSTEM section — no duplicates here (spec §5).
function UserMenu({ user, onLogout }: { user: SessionUser; onLogout: () => void }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  // Outside-click + Escape close via document listeners.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex h-7 items-center gap-2 rounded-md px-2 transition-colors duration-[120ms] hover:bg-hover"
      >
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-success" aria-hidden />
        <span className="hidden max-w-[16rem] truncate font-mono text-[11px] text-muted sm:block">
          {user.email ?? user.name ?? 'Account'}
        </span>
      </button>
      {open && (
        <div className="gd-enter absolute right-0 top-full z-30 mt-2 w-64 rounded-[10px] border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]">
          <div className="flex items-center gap-2.5 border-b border-line px-3 py-2.5">
            <Avatar src={user.picture} name={user.name ?? user.email} className="h-8 w-8 shrink-0" />
            <div className="min-w-0">
              <div className="truncate text-sm font-medium text-fg">{user.name ?? user.email}</div>
              <div className="flex items-baseline gap-2">
                <span className="min-w-0 truncate font-mono text-[11px] text-muted">{user.email}</span>
                {user.role === 'admin' && (
                  <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-accent">admin</span>
                )}
              </div>
            </div>
          </div>

          <div className="flex items-center justify-between rounded-md px-3 py-1.5 text-sm text-muted">
            <span className="flex items-center gap-2.5">
              <SunMoon size={15} className="shrink-0 text-muted" />
              <span>Theme</span>
            </span>
            <ThemeToggle />
          </div>

          <div className="mt-1 border-t border-line pt-1">
            <button
              type="button"
              onClick={onLogout}
              className="flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-sm text-[color:var(--theme-danger)] transition-colors duration-[120ms] hover:bg-[color:var(--theme-danger)]/10"
            >
              <LogOut size={15} className="shrink-0" />
              <span>Sign out</span>
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

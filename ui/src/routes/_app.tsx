import { createFileRoute, Link, Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { Brand } from '@/components/brand'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { ThemeToggle } from '@/components/theme-toggle'
import { NavRail } from '@/components/app/nav-rail'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { Avatar } from '@/components/ui/avatar'
import { useDeniedViews, useLogout, useSession, type SessionUser } from '@/lib/session'
import { ADMIN_VIEWS } from '@/lib/nav'

// Authenticated app shell: header + left nav rail + the active view (Outlet).
export const Route = createFileRoute('/_app')({
  component: AppLayout,
})

function AppLayout() {
  const { data: user, isLoading, isSuccess } = useSession()
  const denied = useDeniedViews()
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const navigate = useNavigate()
  const logout = useLogout()

  useEffect(() => {
    if (isSuccess && !user) void navigate({ to: '/login' })
  }, [isSuccess, user, navigate])

  // Native context menus are suppressed app-wide — Talaria surfaces provide
  // their own. Editable fields keep the native menu (paste, spellcheck,
  // dictionary) — taking that away breaks real workflows.
  useEffect(() => {
    const onCtx = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null
      if (t?.closest('input, textarea, [contenteditable="true"], [contenteditable=""]')) return
      e.preventDefault()
    }
    document.addEventListener('contextmenu', onCtx)
    return () => document.removeEventListener('contextmenu', onCtx)
  }, [])

  // Route gate: a denied or role-gated view isn't just hidden from the nav —
  // reaching it by URL bounces to Home. (Match prefixes, e.g. /boards/x.)
  useEffect(() => {
    if (!user) return
    const blocked = user.role === 'admin' ? denied : [...denied, ...ADMIN_VIEWS]
    if (blocked.some((v) => pathname === v || pathname.startsWith(v + '/'))) {
      void navigate({ to: '/' })
    }
  }, [user, denied, pathname, navigate])

  if (isLoading || !user) {
    // The session gate used to blank the WHOLE app ("no content, then BAM").
    // Render the real chrome immediately — brand, header, a skeleton nav and
    // page — so the frame is stable and only content fills in.
    return (
      <>
        <MercuryBackdrop />
        <div className="flex h-screen flex-col">
          <header className="relative z-40 flex items-center justify-between gap-3 border-b border-line-subtle px-6 py-3 backdrop-blur">
            <Brand />
            <Skeleton className="h-8 w-8 rounded-full" />
          </header>
          <div className="flex min-h-0 flex-1">
            <nav className="flex h-full w-56 shrink-0 flex-col gap-6 overflow-y-auto border-r border-line-subtle bg-sidebar px-3 py-5">
              {[0, 1, 2].map((g) => (
                <div key={g} className="space-y-2 px-2">
                  <Skeleton className="h-2 w-14 rounded-full" delay={g * 0.1} />
                  <SkeletonRows rows={4} />
                </div>
              ))}
            </nav>
            <div className="min-h-0 min-w-0 flex-1 overflow-hidden p-8">
              <div className="mx-auto max-w-6xl space-y-6">
                <Skeleton className="h-6 w-64 rounded-full" />
                <div className="grid gap-4 xl:grid-cols-3">
                  {[0, 1, 2].map((i) => (
                    <div key={i} className="mercury-panel rounded-2xl p-6">
                      <Skeleton className="mb-4 h-3 w-24 rounded-full" delay={i * 0.1} />
                      <SkeletonRows rows={4} />
                    </div>
                  ))}
                </div>
              </div>
            </div>
          </div>
        </div>
      </>
    )
  }

  return (
    <>
      <MercuryBackdrop />
      <div className="flex h-screen flex-col">
        {/* z-40: the header (and the user-menu flyover inside it) stacks above
            all page content and its popovers; modals portal to body at z-50. */}
        <header className="relative z-40 flex items-center justify-between gap-3 border-b border-line-subtle px-6 py-3 backdrop-blur">
          <Brand />
          <UserMenu user={user} onLogout={() => void logout()} />
        </header>

        <div className="flex min-h-0 flex-1">
          <NavRail user={user} />
          <div className="min-h-0 min-w-0 flex-1 overflow-hidden">
            <Outlet />
          </div>
        </div>
      </div>
    </>
  )
}

// The header's only control: the user chip, with everything personal nested in
// a flyover (profile, settings, theme, sign out) — the menu bar stays clean.
function UserMenu({ user, onLogout }: { user: SessionUser; onLogout: () => void }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  // Outside-click + Escape close via document listeners. (A fixed inset-0
  // overlay can't work here: the header's backdrop-filter makes it the
  // containing block for fixed descendants, so an overlay would only cover
  // the header strip, not the page.)
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
        className="flex items-center gap-2 rounded-full border border-line bg-card py-1 pl-1 pr-3 transition-colors hover:border-line-subtle"
      >
        <Avatar src={user.picture} name={user.name ?? user.email} />
        <span className="hidden max-w-[12rem] truncate text-sm text-fg sm:block">{user.name ?? user.email}</span>
        <span className="text-xs text-muted">▾</span>
      </button>
      {open && (
        <div className="mercury-panel absolute right-0 top-full z-30 mt-2 w-60 rounded-xl p-1">
            <div className="border-b border-line-subtle px-3 py-2.5">
              <div className="truncate text-sm font-medium text-fg">{user.name ?? user.email}</div>
              <div className="flex items-baseline gap-2">
                <span className="min-w-0 truncate text-xs text-muted">{user.email}</span>
                {user.role === 'admin' && <span className="shrink-0 text-xs text-accent">admin</span>}
              </div>
            </div>
            <Link
              to="/settings"
              onClick={() => setOpen(false)}
              className="block rounded-lg px-3 py-2 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
            >
              Settings
            </Link>
            {user.role === 'admin' && (
              <Link
                to="/admin"
                onClick={() => setOpen(false)}
                className="block rounded-lg px-3 py-2 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
              >
                Admin
              </Link>
            )}
            <div className="flex items-center justify-between rounded-lg px-3 py-1.5 text-sm text-muted">
              <span>Theme</span>
              <ThemeToggle />
            </div>
          <button
            type="button"
            onClick={onLogout}
            className="block w-full rounded-lg px-3 py-2 text-left text-sm text-muted transition-colors hover:bg-card hover:text-fg"
          >
            Sign out
          </button>
        </div>
      )}
    </div>
  )
}

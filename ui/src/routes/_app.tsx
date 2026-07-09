import { createFileRoute, Link, Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Brand } from '@/components/brand'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { ThemeToggle } from '@/components/theme-toggle'
import { NavRail } from '@/components/app/nav-rail'
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
    return (
      <>
        <MercuryBackdrop />
        <div className="grid min-h-screen place-items-center text-sm text-muted">Loading…</div>
      </>
    )
  }

  return (
    <>
      <MercuryBackdrop />
      <div className="flex h-screen flex-col">
        <header className="flex items-center justify-between gap-3 border-b border-line-subtle px-6 py-3 backdrop-blur">
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
  return (
    <div className="relative">
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
        <>
          <div className="fixed inset-0 z-20" onClick={() => setOpen(false)} />
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
        </>
      )}
    </div>
  )
}

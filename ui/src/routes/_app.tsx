import { createFileRoute, Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { useEffect } from 'react'
import { Brand } from '@/components/brand'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { ThemeToggle } from '@/components/theme-toggle'
import { NavRail } from '@/components/app/nav-rail'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { useDeniedViews, useLogout, useSession } from '@/lib/session'

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

  // Route gate: a denied view isn't just hidden from the nav — reaching it by
  // URL bounces to Home. (Match the deepest denied prefix, e.g. /boards/x.)
  useEffect(() => {
    if (user && denied.some((v) => pathname === v || pathname.startsWith(v + '/'))) {
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
          <div className="flex items-center gap-3">
            <ThemeToggle />
            <div className="flex items-center gap-2 rounded-full border border-line bg-card py-1 pl-1 pr-3">
              <Avatar src={user.picture} name={user.name ?? user.email} />
              <span className="hidden max-w-[12rem] truncate text-sm text-fg sm:block">
                {user.name ?? user.email}
              </span>
              {user.role === 'admin' && <span className="text-xs text-accent">admin</span>}
            </div>
            <Button variant="ghost" size="sm" onClick={() => void logout()}>
              Sign out
            </Button>
          </div>
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

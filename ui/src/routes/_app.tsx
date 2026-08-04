import { createFileRoute, Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { useEffect, type ReactNode } from 'react'
import { Brand, WingMark } from '@/components/brand'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { NavRail, useNavCollapsed } from '@/components/app/nav-rail'
import { TopStrip } from '@/components/app/top-strip'
import { InboxFocusShell } from '@/components/inbox/inbox-focus-shell'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { ThemeToggle } from '@/components/theme-toggle'
import { useDeniedViews, useLogout, useSession } from '@/lib/session'
import { ADMIN_VIEWS } from '@/lib/nav'
import { shouldAttachInboxDecision } from '@/lib/inbox-focus-surface'

// Authenticated app shell (Mercury, spec §5–6): the collapsible nav rail
// spans the full height on the left; the top strip sits above the active view
// (Outlet) on the right. The brand lives in the rail, not the strip.
export const Route = createFileRoute('/_app')({
  component: AppLayout,
})

function AppLayout() {
  const session = useSession()
  const { data: user, isLoading, isSuccess } = session
  const denied = useDeniedViews()
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const tab = useRouterState({ select: (s) => (s.location.search as { tab?: string }).tab })
  const navigate = useNavigate()
  const logout = useLogout()

  // Only a SUCCESSFUL session read saying "nobody is signed in" sends anyone to
  // /login. /api/auth/session answers 200 with `{ user: null }` when you're
  // signed out, so a non-2xx means the backend blipped — and `isSuccess` is
  // false for it now, which is the whole point: a blip is not a logout.
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

  // The session read FAILED (not "signed out" — that is a 200 with a null user).
  // Say so inside the real chrome and offer a retry. Shimmering forever would be
  // the same lie in slower motion, and bouncing to /login — what a swallowed 500
  // used to do — is worse still: it reads as "you have been signed out".
  if (session.isError && !user) {
    return (
      <ShellSkeleton>
        <QueryError
          error={session.error}
          title="Could not reach your session"
          onRetry={() => void session.refetch()}
        />
      </ShellSkeleton>
    )
  }

  if (isLoading || !user) return <ShellSkeleton />

  return (
    <>
      <MercuryBackdrop />
      <div className="flex h-screen">
        <NavRail user={user} />
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <TopStrip user={user} onLogout={() => void logout()} />
          <div className="min-h-0 min-w-0 flex-1 overflow-hidden">
            <InboxFocusShell attachActiveDecision={shouldAttachInboxDecision(pathname, tab)}>
              <Outlet />
            </InboxFocusShell>
          </div>
        </div>
      </div>
    </>
  )
}

// The session gate used to blank the WHOLE app ("no content, then BAM").
// Render the real frame immediately — rail placeholder (honoring the persisted
// collapse state so the frame never jumps), top strip, a skeleton page — and
// only content fills in.
//
// `children` swaps the skeleton page body for a real message: the session read
// broke. Same frame either way, so "still loading" and "this failed" are the
// same shape and only the words differ — and the theme toggle stays reachable
// because that frame is the whole app until the session comes back.
function ShellSkeleton({ children }: { children?: ReactNode }) {
  // Hydration-safe: server snapshot is "expanded"; the persisted client value
  // swaps in right after hydration (same store NavRail uses, so no jump).
  const { collapsed } = useNavCollapsed()
  return (
    <>
      <MercuryBackdrop />
      <div className="flex h-screen">
        {collapsed ? (
          <nav className="flex h-full w-16 shrink-0 flex-col items-center gap-3 border-r border-line bg-sidebar pb-5 pt-3">
            <div className="grid h-9 w-9 place-items-center">
              <WingMark className="h-5 w-5" />
            </div>
            {[0, 1, 2, 3, 4, 5].map((i) => (
              <Skeleton key={i} className="h-9 w-9 rounded-md" delay={i * 0.08} />
            ))}
          </nav>
        ) : (
          <nav className="flex h-full w-[208px] shrink-0 flex-col gap-5 border-r border-line bg-sidebar px-3 pb-4 pt-5">
            <div className="flex h-6 items-center">
              <Brand />
            </div>
            {[0, 1, 2].map((g) => (
              <div key={g} className="space-y-2 px-2">
                <Skeleton className="h-2 w-14 rounded-full" delay={g * 0.1} />
                <SkeletonRows rows={4} />
              </div>
            ))}
          </nav>
        )}
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <header className="flex h-12 shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4">
            <Skeleton className="h-2.5 w-56 rounded-full" />
            {children ? <ThemeToggle /> : <Skeleton className="h-5 w-40 rounded-full" delay={0.15} />}
          </header>
          <div className="min-h-0 min-w-0 flex-1 overflow-hidden p-8">
            {children ?? (
              <div className="mx-auto max-w-6xl space-y-6">
                <Skeleton className="h-6 w-64 rounded-full" />
                <div className="grid gap-4 xl:grid-cols-3">
                  {[0, 1, 2].map((i) => (
                    <div key={i} className="rounded-lg border border-line bg-panel p-6">
                      <Skeleton className="mb-4 h-3 w-24 rounded-full" delay={i * 0.1} />
                      <SkeletonRows rows={4} />
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </>
  )
}

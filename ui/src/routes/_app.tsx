import { createFileRoute, Link, Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { LogOut, Settings, ShieldCheck, SunMoon } from 'lucide-react'
import { Brand } from '@/components/brand'
import { MercuryBackdrop } from '@/components/mercury-backdrop'
import { ThemeToggle } from '@/components/theme-toggle'
import { NavRail } from '@/components/app/nav-rail'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { Avatar } from '@/components/ui/avatar'
import { useDeniedViews, useLogout, useSession, type SessionUser } from '@/lib/session'
import { ADMIN_VIEWS } from '@/lib/nav'

// Authenticated app shell: header + left nav rail + the active view (Outlet).
export const Route = createFileRoute('/_app')({
  component: AppLayout,
})

function AppLayout() {
  const session = useSession()
  const { data: user, isLoading, isSuccess } = session
  const denied = useDeniedViews()
  const pathname = useRouterState({ select: (s) => s.location.pathname })
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
      <>
        <MercuryBackdrop />
        <div className="flex h-screen flex-col">
          <header className="relative z-40 flex items-center justify-between gap-3 border-b border-line-subtle px-6 py-3 backdrop-blur">
            <Brand />
            <ThemeToggle />
          </header>
          <div className="min-h-0 flex-1">
            <QueryError
              error={session.error}
              title="Could not reach your session"
              onRetry={() => void session.refetch()}
            />
          </div>
        </div>
      </>
    )
  }

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
        <div className="mercury-panel absolute right-0 top-full z-30 mt-2 w-64 rounded-xl p-1">
          <div className="flex items-center gap-2.5 border-b border-line-subtle px-3 py-2.5">
            <Avatar src={user.picture} name={user.name ?? user.email} className="h-8 w-8 shrink-0" />
            <div className="min-w-0">
              <div className="truncate text-sm font-medium text-fg">{user.name ?? user.email}</div>
              <div className="flex items-baseline gap-2">
                <span className="min-w-0 truncate text-xs text-muted">{user.email}</span>
                {user.role === 'admin' && <span className="shrink-0 text-xs text-accent">admin</span>}
              </div>
            </div>
          </div>

          <div className="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wider text-muted">Account</div>
          <Link
            to="/settings"
            onClick={() => setOpen(false)}
            className="flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
          >
            <Settings size={15} className="shrink-0 text-muted" />
            <span>Settings</span>
          </Link>
          <div className="flex items-center justify-between rounded-lg px-3 py-1.5 text-sm text-muted">
            <span className="flex items-center gap-2.5">
              <SunMoon size={15} className="shrink-0 text-muted" />
              <span>Theme</span>
            </span>
            <ThemeToggle />
          </div>

          {user.role === 'admin' && (
            <>
              <div className="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wider text-muted">Administration</div>
              <Link
                to="/admin"
                onClick={() => setOpen(false)}
                className="flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm text-muted transition-colors hover:bg-card hover:text-fg"
              >
                <ShieldCheck size={15} className="shrink-0 text-muted" />
                <span>Admin</span>
              </Link>
            </>
          )}

          <div className="mt-1 border-t border-line-subtle pt-1">
            <button
              type="button"
              onClick={onLogout}
              className="flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm text-[color:var(--theme-danger)] transition-colors hover:bg-[color:var(--theme-danger)]/10"
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

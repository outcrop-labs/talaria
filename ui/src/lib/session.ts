// Client-side session + provider hooks (thin wrappers over the auth API).
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getJson } from '@/lib/fetch-json'
import type { ProviderMeta } from '@/server/auth/config'

export interface SessionUser {
  id: string
  sub: string
  email: string | null
  name: string | null
  picture: string | null
  provider: 'google' | 'password'
  role: 'admin' | 'member'
}

export function useIsAdmin(user: SessionUser | null | undefined): boolean {
  return user?.role === 'admin'
}

interface SessionResult {
  user: SessionUser | null
  deniedViews: string[]
  perms: string[]
}

// SIGNED OUT IS A 200. /api/auth/session always answers 200 and puts the
// answer in the body: `{ user: null }` means "nobody is signed in". So a
// non-2xx from this route is NEVER a logout — it is the session service or the
// database failing. This used to resolve as `{ user: null }`, which _app.tsx
// reads as "signed out" and bounces to /login: a backend blip logged everyone
// out. Now it rejects, the query enters its error state, and nobody moves.
//
// The same rule applies INSIDE a 200: `user: null` is the answer "nobody is
// signed in", but a body with no `user` field at all is a broken contract, and
// reading the absent key as null signs the person out just as wrongly as a 500
// used to. getList already refuses to read a missing wrapper key as an empty
// list; this refuses to read a missing `user` key as a logout.
const fetchSession = async (): Promise<SessionResult> => {
  const data = await getJson<{ user?: SessionUser | null; deniedViews?: string[]; perms?: string[] }>('/api/auth/session')
  if (data.user === undefined) throw new Error('malformed response from /api/auth/session: no "user" field')
  return { user: data.user, deniedViews: data.deniedViews ?? [], perms: data.perms ?? [] }
}

export function useSession() {
  return useQuery({ queryKey: ['session'], queryFn: fetchSession, select: (d) => d.user })
}

/** True while the effective permission set includes `perm`. Admins hold every
 *  permission. Defaults false until the session resolves — affordances appear,
 *  never flash-then-vanish. */
export function useHasPerm(perm: string): boolean {
  const { data } = useQuery({ queryKey: ['session'], queryFn: fetchSession, select: (d) => d.perms })
  return (data ?? []).includes(perm)
}

/** Views the current user may NOT reach (empty for admins / open access). */
export function useDeniedViews(): string[] {
  const { data } = useQuery({ queryKey: ['session'], queryFn: fetchSession, select: (d) => d.deniedViews })
  return data ?? []
}

export function useProviders() {
  return useQuery({
    queryKey: ['auth-providers'],
    // `configured: false` is a real server answer ("no provider is set up"),
    // not a stand-in for a failed request — telling someone their auth is
    // unconfigured when the box is merely down sends them to the wrong fix.
    queryFn: (): Promise<{ providers: ProviderMeta[]; configured: boolean }> =>
      getJson<{ providers: ProviderMeta[]; configured: boolean }>('/api/auth/providers'),
    staleTime: 60_000,
  })
}

export function useLogout() {
  const qc = useQueryClient()
  return async () => {
    await fetch('/api/auth/logout', { method: 'POST', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: ['session'] })
  }
}

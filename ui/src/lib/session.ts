// Client-side session + provider hooks (thin wrappers over the auth API).
// Like the query hooks everywhere in lib/, these call createQuery and so must
// run during component init (same rule React hooks had).
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { errorMessage, getJson, postJson } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'
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
  return createQuery(() => ({ queryKey: ['session'], queryFn: fetchSession, select: (d: SessionResult) => d.user }))
}

/** True while the effective permission set includes `perm`. Admins hold every
 *  permission. Defaults false until the session resolves — affordances appear,
 *  never flash-then-vanish.
 *
 *  Returns a `{ current }` box, not a bare boolean: a primitive computed once
 *  at init would freeze before the session resolves. Read `.current` where the
 *  answer is used (template or `$derived`). */
export function useHasPerm(perm: string): { readonly current: boolean } {
  const query = createQuery(() => ({ queryKey: ['session'], queryFn: fetchSession, select: (d: SessionResult) => d.perms }))
  return {
    get current() {
      return (query.data ?? []).includes(perm)
    },
  }
}

/** Views the current user may NOT reach (empty for admins / open access).
 *  Same `{ current }` box as `useHasPerm`, for the same reason. */
export function useDeniedViews(): { readonly current: string[] } {
  const query = createQuery(() => ({ queryKey: ['session'], queryFn: fetchSession, select: (d: SessionResult) => d.deniedViews }))
  return {
    get current() {
      return query.data ?? []
    },
  }
}

export function useProviders() {
  return createQuery(() => ({
    queryKey: ['auth-providers'],
    // `configured: false` is a real server answer ("no provider is set up"),
    // not a stand-in for a failed request — telling someone their auth is
    // unconfigured when the box is merely down sends them to the wrong fix.
    queryFn: (): Promise<{ providers: ProviderMeta[]; configured: boolean }> =>
      getJson<{ providers: ProviderMeta[]; configured: boolean }>('/api/auth/providers'),
    staleTime: 60_000,
  }))
}

export function useLogout() {
  const qc = useQueryClient()
  return async () => {
    // The nav button fires and forgets this, so a refused sign-out surfaces
    // here rather than as an unhandled rejection — and the session is
    // re-checked either way, because the person is still signed in.
    await postJson<{ ok: true }>('/api/auth/logout').catch((e: unknown) =>
      pushToast({ title: 'Sign out failed', body: errorMessage(e), tone: 'danger' }),
    )
    await qc.invalidateQueries({ queryKey: ['session'] })
  }
}

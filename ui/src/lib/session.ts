// Client-side session + provider hooks (thin wrappers over the auth API).
import { useQuery, useQueryClient } from '@tanstack/react-query'
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
}

const fetchSession = async (): Promise<SessionResult> => {
  const res = await fetch('/api/auth/session', { credentials: 'same-origin' })
  if (!res.ok) return { user: null, deniedViews: [] }
  const data = (await res.json()) as { user: SessionUser | null; deniedViews?: string[] }
  return { user: data.user, deniedViews: data.deniedViews ?? [] }
}

export function useSession() {
  return useQuery({ queryKey: ['session'], queryFn: fetchSession, select: (d) => d.user })
}

/** Views the current user may NOT reach (empty for admins / open access). */
export function useDeniedViews(): string[] {
  const { data } = useQuery({ queryKey: ['session'], queryFn: fetchSession, select: (d) => d.deniedViews })
  return data ?? []
}

export function useProviders() {
  return useQuery({
    queryKey: ['auth-providers'],
    queryFn: async (): Promise<{ providers: ProviderMeta[]; configured: boolean }> => {
      const res = await fetch('/api/auth/providers', { credentials: 'same-origin' })
      if (!res.ok) return { providers: [], configured: false }
      return res.json()
    },
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

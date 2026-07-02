// Directory of signed-in users — powers the people pickers.
import { useQuery } from '@tanstack/react-query'

export interface DirectoryUser {
  id: string
  email: string | null
  name: string | null
}

export function useUsers() {
  return useQuery({
    queryKey: ['users'],
    queryFn: async (): Promise<DirectoryUser[]> => {
      const r = await fetch('/api/users', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { users: DirectoryUser[] }).users
    },
    staleTime: 30_000,
  })
}

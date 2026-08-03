// Directory of signed-in users — powers the people pickers.
import { useQuery } from '@tanstack/react-query'
import { getList } from '@/lib/fetch-json'

export interface DirectoryUser {
  id: string
  email: string | null
  name: string | null
}

export function useUsers() {
  return useQuery({
    queryKey: ['users'],
    // An empty directory in a people picker reads as "nobody works here".
    queryFn: (): Promise<DirectoryUser[]> => getList<DirectoryUser>('/api/users', 'users'),
    staleTime: 30_000,
  })
}

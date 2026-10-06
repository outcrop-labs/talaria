// Directory of signed-in users — powers the people pickers.
import { createQuery } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'

export interface DirectoryUser {
  id: string
  email: string | null
  name: string | null
  /** Effective avatar URL — the uploaded photo when set, else the Google
   *  picture. Absent on servers that predate it. */
  picture?: string | null
  /** Presence heartbeat seen within its TTL. */
  online?: boolean
  statusEmoji?: string | null
  statusText?: string | null
  /** Job title ("Product designer"), set in Settings → Profile. */
  title?: string | null
  /** Their IANA zone — the profile drawer's "local time". */
  timezone?: string | null
}

export function useUsers() {
  return createQuery(() => ({
    queryKey: ['users'],
    // An empty directory in a people picker reads as "nobody works here".
    queryFn: (): Promise<DirectoryUser[]> => getList<DirectoryUser>('/api/users', 'users'),
    staleTime: 30_000,
    // Presence dots ride the directory: poll so they refresh while Comms stays
    // open (TanStack pauses this in background tabs).
    refetchInterval: 30_000,
  }))
}

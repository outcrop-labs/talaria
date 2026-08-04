// The user's notification inbox — polled; refetches piggyback on route changes.
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getJson } from '@/lib/fetch-json'

export interface Notification {
  id: string
  kind: string
  title: string
  body: string
  href: string
  readAt: string | null
  createdAt: string
}

export function useNotifications() {
  return useQuery({
    queryKey: ['notifications'],
    queryFn: (): Promise<{ notifications: Notification[]; unread: number }> =>
      getJson<{ notifications: Notification[]; unread: number }>('/api/notifications'),
    refetchInterval: 30_000,
  })
}

export function useMarkNotificationsRead() {
  const qc = useQueryClient()
  return async (ids?: string[]) => {
    await fetch('/api/notifications', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(ids ? { ids } : {}),
    })
    await qc.invalidateQueries({ queryKey: ['notifications'] })
  }
}

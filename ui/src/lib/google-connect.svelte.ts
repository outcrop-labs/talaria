// The caller's Google connection status — ONE shared cache entry
// ('integration-google') behind every reader: Settings › Assistant,
// Settings › Connections, the home cockpit's status probe, and the Inbox
// assistant panel's connect link. One fetch per surface, invalidated in one
// place after connect/disconnect, so no reader ever shows a stale verdict.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface GoogleConnectStatus {
  available: boolean
  connected: boolean
  email: string | null
  scope: string[]
  connectedAt: string | null
}

export function useGoogleConnectStatus() {
  return createQuery(() => ({
    queryKey: ['integration-google'],
    // A failed read is a non-answer, not "not connected" — `connected: false`
    // on an error would render connect buttons that mislead.
    queryFn: (): Promise<GoogleConnectStatus> => getJson<GoogleConnectStatus>('/api/integrations/google'),
  }))
}

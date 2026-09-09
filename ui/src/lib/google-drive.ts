// The Google Drive client — the browse surface's whole vocabulary. The
// connection state lives server-side (google_connections / org connection);
// every 409 body here is the ladder's (not_connected | reconnect_needed),
// which the Drive place renders as its connect screen.

import { createQuery } from '@tanstack/svelte-query'
import { getJson, getJsonOr, postJsonOr } from '@/lib/fetch-json'

export interface DriveEntry {
  id: string
  name: string
  mimeType: string
  modifiedTime: string | null
  iconLink: string | null
  webViewLink: string | null
  sizeBytes: number | null
}

export interface DriveRosterEntry {
  /** `<connection>:<drive id>` — the wire identity the URL's `d=` carries. */
  key: string
  connection: 'personal' | 'org'
  kind: 'my' | 'shared'
  id: string
  name: string
  writable: boolean
  email: string | null
}

export interface DriveBrowse {
  files: DriveEntry[]
  nextPageToken: string | null
  path: { id: string; name: string }[]
}

/** Every Drive this person can browse. A 409 body is the connect screen's. */
export const useDriveRoster = () =>
  createQuery(() => ({
    queryKey: ['google-drive', 'roster'],
    // 409 is a legitimate ANSWER (not connected) — read it as data.
    queryFn: (): Promise<{ drives?: DriveRosterEntry[]; error?: string }> =>
      getJsonOr('/api/integrations/google/drive/drives', [409]),
    staleTime: 60_000,
  }))

export const importDriveFile = (fileId: string, folderId?: string | null) =>
  postJsonOr<{ artifact?: { id: string }; message?: string }>(
    '/api/integrations/google/drive/import',
    { fileId, ...(folderId ? { folderId } : {}) },
    [409, 413, 502],
  )

/** One folder page — first page (no token) or a Load-more (with). */
export function browseDrivePage(
  driveKey: string,
  folderId: string | null,
  q: string,
  sort: string,
  pageToken?: string,
): Promise<DriveBrowse> {
  const params = new URLSearchParams({ d: driveKey, pageSize: '100', sort })
  if (folderId) params.set('parent', folderId)
  if (q.trim()) params.set('q', q.trim())
  if (pageToken) params.set('pageToken', pageToken)
  return getJson<DriveBrowse>(`/api/integrations/google/drive/browse?${params}`)
}

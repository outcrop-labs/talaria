// Is the Google side actually usable? OAuth consent succeeds even when the
// project's APIs are disabled — Google only checks at call time — so a freshly
// connected org account still 403s on the first Drive export / Calendar read /
// Gmail fetch until someone flips the three library switches in Google Cloud
// Console. This module makes that checkable on demand from the Admin UI
// instead of discovering it one broken surface at a time.

import { getOrgAccessToken } from './org-connection'
import { GOOGLE_API_LIBRARY, type GoogleApiHealth, type GoogleApiService } from '@/lib/google-apis'

// One cheapest-possible authenticated call per service, chosen to stay inside
// the scopes WORKSPACE_SCOPES already grants (no extra consent to test).
const PROBES: Record<GoogleApiService, string> = {
  // One file of the root listing — drive.readonly grants it, and unlike
  // about.get it has no required params to get wrong.
  drive: 'https://www.googleapis.com/drive/v3/files?pageSize=1&fields=files(id)',
  // One event of the primary calendar — calendar.events grants exactly this.
  calendar: 'https://www.googleapis.com/calendar/v3/calendars/primary/events?maxResults=1&fields=nextPageToken',
  // The Gmail profile (mailbox address + history id) — gmail.modify covers it.
  gmail: 'https://www.googleapis.com/gmail/v1/users/me/profile',
}

interface GoogleErrorBody {
  error?: {
    message?: string
    status?: string
    errors?: Array<{ reason?: string }>
    details?: Array<{ reason?: string }>
  }
}

/** Google signals "you never enabled this API" several ways depending on the
 *  service's vintage: SERVICE_DISABLED in ErrorInfo details, accessNotConfigured
 *  in legacy errors[], or just a sentence about it in the message. */
function isDisabled(status: number, body: GoogleErrorBody): boolean {
  if (status !== 403 && status !== 404) return false
  const e = body.error
  if (!e) return false
  if (e.details?.some((d) => d.reason === 'SERVICE_DISABLED')) return true
  if (e.errors?.some((x) => x.reason === 'accessNotConfigured')) return true
  return /has not been used in project|it is disabled/i.test(e.message ?? '')
}

async function probe(service: GoogleApiService, name: string, consoleUrl: string, token: string): Promise<GoogleApiHealth> {
  const res = await fetch(PROBES[service], { headers: { Authorization: `Bearer ${token}` } })
  if (res.ok) return { service, name, consoleUrl, state: 'ok', detail: null }
  const body = (await res.json().catch(() => ({}))) as GoogleErrorBody
  if (isDisabled(res.status, body)) {
    return { service, name, consoleUrl, state: 'disabled', detail: 'Not enabled in this Google Cloud project.' }
  }
  if (res.status === 401) {
    return { service, name, consoleUrl, state: 'error', detail: 'Google rejected the access token — reconnect the org account.' }
  }
  return {
    service,
    name,
    consoleUrl,
    state: 'error',
    detail: body.error?.message?.slice(0, 200) ?? `Google answered ${res.status}.`,
  }
}

/** Probe all three services with the org connection's access token.
 *  Throws (InvalidGrant etc.) when the org connection itself is dead — the
 *  route maps that to a reconnect prompt. */
export async function probeOrgGoogleApis(nowMs: number): Promise<GoogleApiHealth[]> {
  const token = await getOrgAccessToken(nowMs)
  if (!token) {
    const err = new Error('not_connected')
    err.name = 'GoogleNotConnected'
    throw err
  }
  return Promise.all(GOOGLE_API_LIBRARY.map((a) => probe(a.service, a.name, a.consoleUrl, token)))
}

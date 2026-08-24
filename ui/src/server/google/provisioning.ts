// Org workspace provisioning — the one-click "make the shared Google workspace
// real" pass. Two containers, both created AS the connected org account and
// shared with its Workspace domain so every person there can reach them
// without anyone hand-configuring sharing in Google:
//
//   · an org calendar ("Talaria") — domain-wide WRITER, so members see and
//     edit it; agents already read/write whatever calendar_id points at
//   · a Shared Drive ("Talaria") — domain-wide FILE ORGANIZER (a content
//     manager: add, edit, move, trash), with artifact exports repointed at
//     its root so team files are born team-owned
//
// Every step is idempotent: a stored id is verified against Google before it
// is trusted (a calendar deleted out from under us is recreated, not 404ed on
// forever), and share rules are checked-then-added so re-running the pass
// never duplicates. Outcomes come back per-item — one failed container must
// not hide the other's success — and the route hands them to the Admin UI.

import { emailDomainOf } from './aliasing'
import { getOrgAccessToken, getOrgConnectionStatus, setOrgSharedDrive, setOrgTargets, type OrgTargets } from './org-connection'

const CAL_BASE = 'https://www.googleapis.com/calendar/v3'
const DRIVE_BASE = 'https://www.googleapis.com/drive/v3'

export const ORG_CALENDAR_NAME = 'Talaria'
export const ORG_DRIVE_NAME = 'Talaria'

// Full scopes, not the per-file/per-event ones — see ORG_CONNECT_SCOPES in
// oauth.ts for why provisioning needs the wider grants.
const CALENDAR_SCOPE = 'https://www.googleapis.com/auth/calendar'
const DRIVE_SCOPE = 'https://www.googleapis.com/auth/drive'

export type ProvisionState = 'created' | 'reused' | 'shared'
export type ProvisionFailure = 'not_connected' | 'reconnect_needed' | 'consumer_account' | 'no_domain' | 'google_error'
export type ProvisionResult = { ok: true; state: ProvisionState; id: string } | { ok: false; error: ProvisionFailure; message: string }

/** A failed Google call, carrying the HTTP status so `classify` can read it. */
class GoogleCallError extends Error {
  status: number
  constructor(message: string, status: number) {
    super(message)
    this.status = status
  }
}

/** A Google error message, or a stand-in for a body with none. */
async function errText(res: Response): Promise<string> {
  const body = (await res.json().catch(() => ({}))) as { error?: { message?: string } }
  return body.error?.message ?? `Google answered ${res.status}.`
}

/** The Google error classes this pass distinguishes. Scope/API messages become
 *  reconnect prompts even past the stored-scope preflight (Google can still
 *  refuse a granted scope per-admin); 403s mentioning shared drives mark the
 *  Shared-Drives-need-Workspace wall. */
function classify(err: GoogleCallError): ProvisionFailure {
  const text = `${err.message} ${err.status}`
  if (/insufficient|ACCESS_TOKEN_SCOPE|accessNotConfigured|SERVICE_DISABLED/i.test(text)) return 'reconnect_needed'
  if (err.status === 403 && /shared.?drives?|team.?drives?/i.test(text)) return 'consumer_account'
  return 'google_error'
}

/** Connected org account + the scope a container needs, or the outcome saying
 *  why not. Checking the STORED scope first turns Google's opaque 403 into
 *  "reconnect" — the fix an admin can actually act on. */
async function preflight(scope: string, surface: string): Promise<
  { token: string; domain: string; targets: OrgTargets } | { fail: ProvisionResult }
> {
  const status = await getOrgConnectionStatus()
  if (!status.connected) {
    return { fail: { ok: false, error: 'not_connected', message: 'Connect the org Google account first.' } }
  }
  if (!status.scope.includes(scope)) {
    return {
      fail: {
        ok: false,
        error: 'reconnect_needed',
        message: `Reconnect the org account to grant ${surface} provisioning (one-time, for the wider scopes).`,
      },
    }
  }
  const domain = emailDomainOf(status.email)
  if (!domain) {
    return { fail: { ok: false, error: 'no_domain', message: 'The org account has no usable email address to share with.' } }
  }
  const token = await getOrgAccessToken(Date.now())
  if (!token) return { fail: { ok: false, error: 'not_connected', message: 'Connect the org Google account first.' } }
  return { token, domain, targets: status.targets }
}

// ── Calendar ─────────────────────────────────────────────────────────────────

/** A calendar id that still exists. A calendar deleted in Google (an admin
 *  cleaning up) must be recreated, not trusted forever. */
async function calendarExists(token: string, id: string): Promise<boolean> {
  const res = await fetch(`${CAL_BASE}/calendars/${encodeURIComponent(id)}?fields=id`, {
    headers: { Authorization: `Bearer ${token}` },
  })
  return res.ok
}

/** True when a domain-wide rule for `domain` is already on the calendar. */
async function calendarSharedWithDomain(token: string, id: string, domain: string): Promise<boolean> {
  const res = await fetch(`${CAL_BASE}/calendars/${encodeURIComponent(id)}/acl?fields=items(scope)`, {
    headers: { Authorization: `Bearer ${token}` },
  })
  if (!res.ok) return false // unreadable → let the insert run; duplicates are tolerated below
  const { items = [] } = (await res.json()) as { items?: Array<{ scope?: { type?: string; value?: string } }> }
  return items.some((i) => i.scope?.type === 'domain' && i.scope.value?.toLowerCase() === domain)
}

async function shareCalendarWithDomain(token: string, id: string, domain: string): Promise<void> {
  const res = await fetch(`${CAL_BASE}/calendars/${encodeURIComponent(id)}/acl`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ scope: { type: 'domain', value: domain }, role: 'writer' }),
  })
  // Already shared (a race, or a recreate of a calendar Google still remembers)
  // adds up to the success it was aiming at, not an error.
  if (!res.ok && res.status !== 409) throw new GoogleCallError(await errText(res), res.status)
}

/** Create (or re-use) the org calendar and share it with the Workspace domain. */
export async function provisionOrgCalendar(): Promise<ProvisionResult> {
  const pre = await preflight(CALENDAR_SCOPE, 'calendar')
  if ('fail' in pre) return pre.fail
  const { token, domain, targets } = pre

  try {
    let id = targets.calendarId ?? null
    let state: ProvisionState
    if (id && (await calendarExists(token, id))) {
      state = 'reused'
    } else {
      const res = await fetch(`${CAL_BASE}/calendars`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ summary: ORG_CALENDAR_NAME }),
      })
      if (!res.ok) throw new GoogleCallError(await errText(res), res.status)
      id = ((await res.json()) as { id: string }).id
      state = 'created'
      await setOrgTargets({ calendarId: id })
    }

    if (!(await calendarSharedWithDomain(token, id, domain))) {
      await shareCalendarWithDomain(token, id, domain)
      if (state === 'reused') state = 'shared'
    }
    return { ok: true, state, id }
  } catch (err) {
    return failed('Calendar', err)
  }
}

// ── Shared Drive ─────────────────────────────────────────────────────────────

async function sharedDriveExists(token: string, id: string): Promise<boolean> {
  const res = await fetch(`${DRIVE_BASE}/drives/${encodeURIComponent(id)}?supportsAllDrives=true&fields=id`, {
    headers: { Authorization: `Bearer ${token}` },
  })
  return res.ok
}

/** True when a domain-wide permission is already on the drive. */
async function driveSharedWithDomain(token: string, id: string): Promise<boolean> {
  const res = await fetch(
    `${DRIVE_BASE}/drives/${encodeURIComponent(id)}/permissions?supportsAllDrives=true&fields=permissions(type)`,
    { headers: { Authorization: `Bearer ${token}` } },
  )
  if (!res.ok) return false
  const { permissions = [] } = (await res.json()) as { permissions?: Array<{ type?: string }> }
  return permissions.some((p) => p.type === 'domain')
}

async function shareDriveWithDomain(token: string, id: string): Promise<void> {
  const res = await fetch(`${DRIVE_BASE}/drives/${encodeURIComponent(id)}/permissions?supportsAllDrives=true`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    // fileOrganizer = the shared drive's "Content manager": add, edit, move,
    // trash. The team's members manage the team's files — writer would fence
    // them out of the organizing half of "everyone can access it".
    body: JSON.stringify({ role: 'fileOrganizer', type: 'domain', allowFileDiscovery: true }),
  })
  if (!res.ok && res.status !== 409) throw new GoogleCallError(await errText(res), res.status)
}

/** Create (or re-use) the Shared Drive and grant the Workspace domain access.
 *  Exports are repointed at the drive root — team files, team-owned. */
export async function provisionSharedDrive(): Promise<ProvisionResult> {
  const pre = await preflight(DRIVE_SCOPE, 'Drive')
  if ('fail' in pre) return pre.fail
  const { token, targets } = pre

  try {
    let id = targets.sharedDriveId ?? null
    let state: ProvisionState
    if (id && (await sharedDriveExists(token, id))) {
      state = 'reused'
    } else {
      const res = await fetch(`${DRIVE_BASE}/drives?requestId=${crypto.randomUUID()}`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: ORG_DRIVE_NAME }),
      })
      if (!res.ok) throw new GoogleCallError(await errText(res), res.status)
      id = ((await res.json()) as { id: string }).id
      state = 'created'
      await setOrgSharedDrive(id)
      // The drive is the point — files land in it unless an admin retargets.
      await setOrgTargets({ driveFolderId: id })
    }

    if (!(await driveSharedWithDomain(token, id))) {
      await shareDriveWithDomain(token, id)
      if (state === 'reused') state = 'shared'
    }
    return { ok: true, state, id }
  } catch (err) {
    return failed('Shared Drive', err)
  }
}

/** A thrown Google call → its outcome. The consumer-account wall gets the
 *  sentence an admin can act on; everything else keeps Google's words. */
function failed(surface: string, err: unknown): ProvisionResult {
  const gerr = err instanceof GoogleCallError ? err : null
  const kind = gerr ? classify(gerr) : 'google_error'
  const message =
    kind === 'consumer_account'
      ? 'Shared Drives need a Google Workspace account — the connected org account is a consumer account.'
      : `${surface} provisioning failed: ${(err as Error).message.slice(0, 300)}`
  return { ok: false, error: kind, message }
}

/** Run the requested provisions. Each runs and reports independently; a throw
 *  from one (a dead refresh token) becomes its failure outcome, not a lost
 *  sibling result. */
export async function provisionWorkspace(input: { calendar?: boolean; drive?: boolean }): Promise<{
  calendar?: ProvisionResult
  drive?: ProvisionResult
}> {
  const out: { calendar?: ProvisionResult; drive?: ProvisionResult } = {}
  if (input.calendar) {
    try {
      out.calendar = await provisionOrgCalendar()
    } catch (err) {
      out.calendar = thrownOutcome(err)
    }
  }
  if (input.drive) {
    try {
      out.drive = await provisionSharedDrive()
    } catch (err) {
      out.drive = thrownOutcome(err)
    }
  }
  return out
}

function thrownOutcome(err: unknown): ProvisionResult {
  if ((err as Error).name === 'GoogleNotConnected') {
    return { ok: false, error: 'not_connected', message: 'Connect the org Google account first.' }
  }
  return { ok: false, error: 'google_error', message: `${(err as Error).message}`.slice(0, 300) }
}

/** Scope readiness for the Admin panel: can this connection create each
 *  container, or does it need the one-time wider-scope reconnect? */
export async function provisioningReadiness(): Promise<{
  connected: boolean
  email: string | null
  calendarScope: boolean
  driveScope: boolean
}> {
  const status = await getOrgConnectionStatus()
  return {
    connected: status.connected,
    email: status.email,
    calendarScope: status.scope.includes(CALENDAR_SCOPE),
    driveScope: status.scope.includes(DRIVE_SCOPE),
  }
}

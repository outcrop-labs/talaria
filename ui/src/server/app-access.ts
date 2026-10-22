// Who may reach an app, decided once for both doors that serve one: the app
// API gateway (/api/apps/<slug>/*) and the build door (/app-builds/<slug>/…).
//
// THE SESSION IS RUST'S. The grants a member holds are resolved by
// `GET /api/auth/session` — person grants, team grants and the agent-manager
// grant together. This host used to keep its own copy of that resolution
// (users.ts `deniedViews`), which read only the person's row; it is gone, and
// this module asks the authority with the caller's own cookie instead, so
// the gateway, the nav and the route gate can never disagree again.
//
// THE RULE is app-grants.ts: each surface is its own grant, and a caller
// reaches the app when they hold at least one — read off the session's
// `appGrants`, the app views Rust lists as theirs, never off a view missing
// from `deniedViews`. The app learns which ones (`ctx.access`) so it can
// refuse a manage-only endpoint to a work-only caller itself.
import { appAccess, reachesApp, type AppAccess } from '@/lib/app-grants'
import type { SessionUser } from './auth/session'
import { json } from './http'
import { enabledApps, type AppManifest } from './apps'
import { rustSession, type RustSession } from './rust-session'

export type { AppAccess }

/** A caller the gate admitted to one app. */
export interface AppCaller {
  user: SessionUser
  app: AppManifest
  access: AppAccess
  perms: string[]
}

export type AppGate = { ok: true; caller: AppCaller } | { ok: false; response: Response }

/** The pure half of the gate, given the session and the enabled apps. The
 *  order is the disclosure order: a stranger learns nothing about which apps
 *  exist (401 first); a member learns an app is not there (404) before
 *  learning they lack it (403). */
export function decideAppGate(session: RustSession, apps: readonly AppManifest[], slug: string): AppGate {
  const user = session.user
  if (!user) return { ok: false, response: json({ error: 'unauthorized' }, { status: 401 }) }
  const app = apps.find((a) => a.slug === slug)
  if (!app) return { ok: false, response: json({ error: 'no such app' }, { status: 404 }) }
  const access = appAccess(app, { isAdmin: user.role === 'admin', appGrants: session.appGrants })
  if (!reachesApp(access)) return { ok: false, response: json({ error: 'forbidden' }, { status: 403 }) }
  return { ok: true, caller: { user, app, access, perms: session.perms } }
}

const unavailable = () => json({ error: 'could not check your access to this app — try again' }, { status: 503 })

/** Session, enabled, granted — the trust work both app doors do before
 *  anything of the app's runs or ships. A session or an enabled-app list
 *  that cannot be read answers 503: the gate never guesses. */
export async function authorizeApp(request: Request, slug: string): Promise<AppGate> {
  let session: RustSession
  try {
    session = await rustSession(request)
  } catch (e) {
    console.error(`[apps] ${slug}: could not read the session from the api`, e)
    return { ok: false, response: unavailable() }
  }
  let apps: AppManifest[]
  try {
    apps = await enabledApps()
  } catch (e) {
    console.error(`[apps] ${slug}: could not read the enabled apps`, e)
    return { ok: false, response: unavailable() }
  }
  return decideAppGate(session, apps, slug)
}

/** `ctx.can(perm)`: an admin holds every permission (Rust already lists them
 *  all for an admin; the role check keeps that true if the list ever lags). */
export const canFor = (caller: Pick<AppCaller, 'user' | 'perms'>) => (perm: string): boolean =>
  caller.user.role === 'admin' || caller.perms.includes(perm)

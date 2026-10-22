// Route-guard helpers — the ONE way API handlers authenticate and authorize.
// Every guard returns either the resolved user or a ready-to-return
// 401/403 Response:
//
//   const gate = await requireAdmin(request)
//   if (gate instanceof Response) return gate
//   const user = gate
//
import { json } from '@/server/http'
import type { z } from 'zod'
import { isViewDenied } from '@/lib/app-grants'
import { getSessionUser, type SessionUser } from './auth/session'
import { rustSession } from './rust-session'

export type { SessionUser }

/** Signed-in user or 401. */
export async function requireUser(request: Request): Promise<SessionUser | Response> {
  const user = await getSessionUser(request)
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  return user
}

/** Admin or 401/403. */
export async function requireAdmin(request: Request): Promise<SessionUser | Response> {
  const user = await getSessionUser(request)
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
  return user
}

/** Signed-in user whose view is NOT DENIED, as the Rust api resolves the
 *  caller's grants (person + teams + agent managers, `GET /api/auth/session`
 *  — the one resolution; this host keeps no copy of it). A core view is denied
 *  by itself or a denied view above it ('x' denies 'x/anything'); an app's
 *  work and manage views are separate grants, each open only when the
 *  session lists it in `appGrants` (lib/app-grants.ts). Admins are exempt. The same rule the nav and route
 *  gate use. For an app's own routes use `authorizeApp` (app-access.ts),
 *  which also tells the app which surfaces the caller holds. */
export async function requireView(request: Request, view: string): Promise<SessionUser | Response> {
  let session
  try {
    session = await rustSession(request)
  } catch (e) {
    console.error(`[api-guard] requireView ${view}: could not read the session from the api`, e)
    return json({ error: 'could not check your access — try again' }, { status: 503 })
  }
  const user = session.user
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  if (user.role !== 'admin' && isViewDenied(view, { isAdmin: false, denied: session.deniedViews, appGrants: session.appGrants })) {
    return json({ error: 'forbidden' }, { status: 403 })
  }
  return user
}

/** Validated body or the standard 400. Surfaces the first zod issue — vague
 *  "bad request" responses waste everyone's time. */
export async function parseBody<S extends z.ZodTypeAny>(
  request: Request,
  schema: S,
): Promise<z.infer<S> | Response> {
  const parsed = schema.safeParse(await request.json().catch(() => null))
  if (!parsed.success) {
    return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })
  }
  return parsed.data as z.infer<S>
}

/** Canonical audit-log actor for a session user. */
export const actorOf = (user: SessionUser): string => user.email ?? user.name ?? user.id

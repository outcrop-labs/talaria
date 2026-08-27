// Route-guard helpers — the ONE way API handlers authenticate and authorize.
// Every guard returns either the resolved user or a ready-to-return
// 401/403 Response:
//
//   const gate = await requireAdmin(request)
//   if (gate instanceof Response) return gate
//   const user = gate
//
// requirePerm (permission catalog) lives in ./permissions and follows the
// same contract. parseBody standardizes zod validation + the 400 shape;
// actorOf standardizes audit-actor derivation.
import { json } from '@/server/http'
import type { z } from 'zod'
import { getSessionUser, type SessionUser } from './auth/session'
import { deniedViews } from './users'

export { requirePerm } from './permissions'
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

/** Signed-in user whose view is NOT DENIED. Denial-based, so a member sees a
 *  view by default and only a deniedViews entry (or a prefix of one — 'x'
 *  denies 'x/anything') takes it away; admins are exempt. The same resolution
 *  the nav and route gates use. For APIs that power a gateable view:
 *  /observability, /models, /agents, /apps, /x/<slug>… */
export async function requireView(request: Request, view: string): Promise<SessionUser | Response> {
  const user = await getSessionUser(request)
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  if (user.role !== 'admin') {
    const denied = await deniedViews(user.id, user.role)
    if (denied.some((v) => v === view || view.startsWith(v + '/'))) {
      return json({ error: 'forbidden' }, { status: 403 })
    }
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

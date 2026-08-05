import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import {
  PERMISSIONS,
  getOrgDefaultPerms,
  getUserPermOverrides,
  setOrgDefaultPerm,
  setUserPermOverride,
  type Perm,
} from '@/server/permissions'
import { db } from '@/server/db/pg'
import { logAudit } from '@/server/audit'

const PERM_IDS = PERMISSIONS.map((p) => p.id)

// Fine-grained permissions admin. GET → the catalog + org member defaults +
// every user's overrides. PUT { userId, perm, allowed|null } → set/clear a
// per-user override (null = back to the org default). PUT { orgDefault:
// { perm, enabled|null } } → tune what plain members can do out of the box
// (null = back to the shipped default). Admins only; both paths audit.
export const Route = defineApi('/api/admin/permissions', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const sql = await db()
    const rows = (await sql`select user_id as "userId", perm, allowed from user_permissions`) as unknown as Array<{
      userId: string
      perm: string
      allowed: boolean
    }>
    const overrides: Record<string, Record<string, boolean>> = {}
    for (const r of rows) (overrides[r.userId] ??= {})[r.perm] = r.allowed
    return json({ catalog: PERMISSIONS, orgDefaults: await getOrgDefaultPerms(), overrides })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const actor = actorOf(user)
    const body = await parseBody(
      request,
      z.union([
        z.object({
          userId: z.string().uuid(),
          perm: z.enum(PERM_IDS as [Perm, ...Perm[]]),
          allowed: z.boolean().nullable(),
        }),
        z.object({
          orgDefault: z.object({ perm: z.enum(PERM_IDS as [Perm, ...Perm[]]), enabled: z.boolean().nullable() }),
        }),
      ]),
    )
    if (body instanceof Response) return body

    if ('orgDefault' in body) {
      const { perm, enabled } = body.orgDefault
      await setOrgDefaultPerm(perm, enabled)
      void logAudit({ actor, action: 'permissions.org_default', targetType: 'permission', targetId: perm, after: { enabled } })
      return json({ orgDefaults: await getOrgDefaultPerms() })
    }
    const { userId, perm, allowed } = body
    await setUserPermOverride(userId, perm, allowed)
    void logAudit({ actor, action: 'permissions.user_override', targetType: 'user', targetId: userId, after: { perm, allowed } })
    return json({ overrides: await getUserPermOverrides(userId) })
  },
})

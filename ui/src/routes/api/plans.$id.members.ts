import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid, Email } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { addPlanMember, listPlanMembers, planRole, removePlanMember } from '@/server/conversations'
import { planDocFor } from '@/server/plan-doc'
import { listEditors, setEditors } from '@/server/kb-perms'
import { addNotification } from '@/server/notifications'
import { getRedis } from '@/server/db/redis'
import { db } from '@/server/db/pg'

const PRESENCE_TTL_S = 60
const presenceKey = (planId: string, userId: string) => `plan:presence:${planId}:${userId}`

/** Keep the plan doc's editor grants in step with membership. */
async function syncDocGrant(planId: string, userId: string, present: boolean): Promise<void> {
  const doc = await planDocFor(planId)
  if (!doc) return
  const grants = (await listEditors('artifact', doc.id)).filter(
    (g) => !(g.principalType === 'user' && g.principalId === userId),
  )
  if (present) grants.push({ principalType: 'user', principalId: userId, role: 'editor' })
  await setEditors('artifact', doc.id, grants)
}

// Multiplayer plan membership + presence.
// GET → { members, active } (any member; active = user ids seen in the last
// minute). POST { email } → share (owner only; grants the doc, notifies them).
// DELETE { userId } → unshare (owner, or a collaborator leaving). PUT → presence
// ping (any member).
export const Route = defineApi('/api/plans/$id/members', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await planRole(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })
    const members = await listPlanMembers(params.id)
    const redis = getRedis()
    const flags = await Promise.all(members.map((m) => redis.exists(presenceKey(params.id, m.userId))))
    return json({ members, active: members.filter((_, i) => flags[i] === 1).map((m) => m.userId) })
  },
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if ((await planRole(user.id, params.id)) !== 'owner') {
      return json({ error: 'only the plan owner can share it' }, { status: 403 })
    }
    const body = await parseBody(request, z.object({ email: Email }))
    if (body instanceof Response) return body
    const sql = await db()
    const rows = (await sql`select id from users where lower(email) = ${body.email.toLowerCase()}`) as unknown as Array<{ id: string }>
    if (!rows[0]) return json({ error: 'no user with that email' }, { status: 400 })
    if (rows[0].id === user.id) return json({ error: 'that is you' }, { status: 400 })
    await addPlanMember(params.id, rows[0].id)
    await syncDocGrant(params.id, rows[0].id, true)
    const [plan] = (await sql`select title from conversations where id = ${params.id}`) as unknown as Array<{ title: string | null }>
    void addNotification(rows[0].id, {
      kind: 'plan-share',
      title: `${user.name ?? user.email ?? 'Someone'} added you to a plan`,
      body: plan?.title ?? 'Untitled plan',
      href: `/plan/${params.id}`,
    }).catch(() => {})
    return json({ members: await listPlanMembers(params.id) })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const role = await planRole(user.id, params.id)
    const body = await parseBody(request, z.object({ userId: Uuid }))
    if (body instanceof Response) return body
    // Owner removes anyone; a collaborator may remove only themself (leave).
    if (role !== 'owner' && !(role === 'collaborator' && body.userId === user.id)) {
      return json({ error: 'forbidden' }, { status: 403 })
    }
    await removePlanMember(params.id, body.userId)
    await syncDocGrant(params.id, body.userId, false)
    return json({ members: await listPlanMembers(params.id) })
  },
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await planRole(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })
    await getRedis().set(presenceKey(params.id, user.id), '1', 'EX', PRESENCE_TTL_S)
    return json({ ok: true })
  },
})

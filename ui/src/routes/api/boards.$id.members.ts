import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardAllowsAgent, boardRole, canEdit, listMembers, shareBoard, unshareBoard } from '@/server/boards'
import { db } from '@/server/db/pg'
import { actingUser } from '@/server/users'
import { agentCaller } from '@/server/agent-auth'
import { logAudit } from '@/server/audit'

// GET → members. POST { email, role } → share (owner/editor). DELETE { userId
// | email } → unshare. Write actions accept a personal assistant acting as its
// owner (identity proxy) alongside signed-in humans.
export const Route = defineApi('/api/boards/$id/members', {
  GET: async ({ request, params }) => {
    // Agents allowed on the board may READ membership (they mutate it
    // blind otherwise); writes below stay identity-proxied.
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (agent) {
      // The CALLER, not its model: board policy falls back to the
      // elevated-assistant bypass (org-wide reach), and a bare string reads
      // as proven — `agent.model` would discard `legacy` silently.
      if (!(await boardAllowsAgent(params.id, agent))) return json({ error: 'forbidden' }, { status: 403 })
      return json({ members: await listMembers(params.id) })
    }
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    return json({ members: await listMembers(params.id) })
  },
  POST: async ({ request, params }) => {
    const user = await actingUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (!canEdit(await boardRole(user.id, params.id)) && !user.elevated) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(
      request,
      z.object({ email: z.string().email(), role: z.enum(['editor', 'viewer']).default('editor') }),
    )
    if (body instanceof Response) return body
    const result = await shareBoard(params.id, body.email, body.role)
    if (!result.ok) return json({ error: result.error }, { status: 400 })
    void logAudit({
      actor: user.label,
      action: 'board.member_add',
      targetType: 'board',
      targetId: params.id,
      after: { email: body.email, role: body.role },
    })
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await actingUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (!canEdit(await boardRole(user.id, params.id)) && !user.elevated) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(
      request,
      z
        .object({ userId: z.string().uuid().optional(), email: z.string().email().optional() })
        .refine((b) => b.userId || b.email, { message: 'userId or email required' }),
    )
    if (body instanceof Response) return body
    let userId = body.userId
    if (!userId && body.email) {
      const sql = await db()
      const rows = (await sql`select id from users where lower(email) = ${body.email.toLowerCase()}`) as unknown as Array<{ id: string }>
      if (!rows[0]) return json({ error: 'no user with that email' }, { status: 400 })
      userId = rows[0].id
    }
    await unshareBoard(params.id, userId!)
    void logAudit({
      actor: user.label,
      action: 'board.member_remove',
      targetType: 'board',
      targetId: params.id,
      after: { userId },
    })
    return json({ ok: true })
  },
})

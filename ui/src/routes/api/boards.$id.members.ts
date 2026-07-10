import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { boardAllowsAgent, boardRole, canEdit, listMembers, shareBoard, unshareBoard } from '@/server/boards'
import { db } from '@/server/db/pg'
import { actingUser } from '@/server/users'
import { agentName, checkAgentKey } from '@/server/agent-auth'

// GET → members. POST { email, role } → share (owner/editor). DELETE { userId
// | email } → unshare. Write actions accept a personal assistant acting as its
// owner (identity proxy) alongside signed-in humans.
export const Route = createFileRoute('/api/boards/$id/members')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        // Agents allowed on the board may READ membership (they mutate it
        // blind otherwise); writes below stay identity-proxied.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !(await boardAllowsAgent(params.id, name))) return json({ error: 'forbidden' }, { status: 403 })
          return json({ members: await listMembers(params.id) })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json({ members: await listMembers(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await actingUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canEdit(await boardRole(user.id, params.id)) && !user.elevated) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ email: z.string().email(), role: z.enum(['editor', 'viewer']).default('editor') })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const result = await shareBoard(params.id, parsed.data.email, parsed.data.role)
        return result.ok ? json({ ok: true }) : json({ ok: false, error: result.error }, { status: 400 })
      },
      DELETE: async ({ request, params }) => {
        const user = await actingUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canEdit(await boardRole(user.id, params.id)) && !user.elevated) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ userId: z.string().uuid().optional(), email: z.string().email().optional() })
          .refine((b) => b.userId || b.email, { message: 'userId or email required' })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        let userId = parsed.data.userId
        if (!userId && parsed.data.email) {
          const sql = await db()
          const rows = (await sql`select id from users where lower(email) = ${parsed.data.email.toLowerCase()}`) as unknown as Array<{ id: string }>
          if (!rows[0]) return json({ error: 'no user with that email' }, { status: 400 })
          userId = rows[0].id
        }
        await unshareBoard(params.id, userId!)
        return json({ ok: true })
      },
    },
  },
})

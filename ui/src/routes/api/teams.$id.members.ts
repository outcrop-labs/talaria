import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { addTeamMember, listTeamMembers, removeTeamMember, teamRole } from '@/server/teams'

const Post = z.object({ email: z.string().email(), role: z.enum(['owner', 'member']).default('member') })
const Delete = z.object({ userId: z.string().uuid() })

// GET → members (any member). POST { email, role } → add (owner). DELETE { userId } → remove (owner).
export const Route = createFileRoute('/api/teams/$id/members')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await teamRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json({ members: await listTeamMembers(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if ((await teamRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Post)
        if (body instanceof Response) return body
        const result = await addTeamMember(params.id, body.email, body.role)
        if (!result.ok) return json({ error: result.error }, { status: 400 })
        await logAudit({
          actor: actorOf(user),
          action: 'team.member_add',
          targetType: 'team',
          targetId: params.id,
          after: { email: body.email },
        })
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if ((await teamRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Delete)
        if (body instanceof Response) return body
        await removeTeamMember(params.id, body.userId)
        await logAudit({
          actor: actorOf(user),
          action: 'team.member_remove',
          targetType: 'team',
          targetId: params.id,
          after: { userId: body.userId },
        })
        return json({ ok: true })
      },
    },
  },
})

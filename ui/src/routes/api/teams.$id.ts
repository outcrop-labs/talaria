import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { deleteTeam, renameTeam, teamRole } from '@/server/teams'

const Patch = z.object({ name: z.string().min(1).max(120) })

// PATCH { name } → rename the team (owner). DELETE → delete it (owner); the
// member rows cascade and its boards survive as personal boards (team_id is
// set null, not cascaded), which is why this is owner-gated like every team
// mutation and not merely member-gated like the member read.
export const Route = defineApi('/api/teams/$id', {
  PATCH: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if ((await teamRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    await renameTeam(params.id, body.name)
    await logAudit({
      actor: actorOf(user),
      action: 'team.rename',
      targetType: 'team',
      targetId: params.id,
      after: { name: body.name },
    })
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if ((await teamRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
    await deleteTeam(params.id)
    await logAudit({
      actor: actorOf(user),
      action: 'team.delete',
      targetType: 'team',
      targetId: params.id,
    })
    return json({ ok: true })
  },
})

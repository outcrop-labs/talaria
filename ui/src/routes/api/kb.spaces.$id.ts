import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { deleteSpace, getSpace, updateSpace } from '@/server/kb'
import { canEditHuman, canRead, listEditors, setEditors, canGovern } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Editor = z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1).max(200), role: z.enum(['viewer', 'editor']).default('viewer') })
const Patch = z.object({
  name: z.string().min(1).max(80).optional(),
  description: z.string().max(400).nullish(),
  icon: z.string().max(16).nullish(),
  body: z.string().max(500_000).optional(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  editPolicy: z.enum(['owner', 'org', 'restricted']).optional(),
  editors: z.array(Editor).max(200).optional(),
})

// One KB folder (space). Same permission model as docs: read gated by
// visibility, writes by the edit policy + editor grants, sharing owner-only.
export const Route = defineApi('/api/kb/spaces/$id', {
  GET: async ({ request, params }) => {
    const space = await getSpace(params.id)
    if (!space) return json({ error: 'not found' }, { status: 404 })
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const editors = await listEditors('space', space.id)
    if (!canRead(space, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
    return json({ space, editors })
  },
  PUT: async ({ request, params }) => {
    const space = await getSpace(params.id)
    if (!space) return json({ error: 'not found' }, { status: 404 })
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    const editors = await listEditors('space', space.id)
    if (!canEditHuman(space, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
    const owner = await canGovern(space, user)
    if (!owner && (body.visibility !== undefined || body.editPolicy !== undefined || body.editors !== undefined)) {
      return json({ error: 'only the owner can change sharing' }, { status: 403 })
    }
    if (owner && body.editors !== undefined) await setEditors('space', params.id, body.editors)
    const updated = await updateSpace(params.id, body, user.email ?? user.name ?? 'user')
    return json({ space: updated, editors: await listEditors('space', params.id) })
  },
  DELETE: async ({ request, params }) => {
    const space = await getSpace(params.id)
    if (!space) return json({ error: 'not found' }, { status: 404 })
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const editors = await listEditors('space', space.id)
    if (!canEditHuman(space, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
    await deleteSpace(params.id)
    void logAudit({ actor: actorOf(user), action: 'kb.space.delete', targetType: 'kb-space', targetId: params.id })
    return json({ ok: true })
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteSpace, getSpace, updateSpace } from '@/server/kb'
import { canEditHuman, canRead, isOwner, listEditors, setEditors } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Editor = z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1).max(200) })
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
export const Route = createFileRoute('/api/kb/spaces/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const space = await getSpace(params.id)
        if (!space) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canRead(space, user.id, user.email ?? user.name)) return json({ error: 'forbidden' }, { status: 403 })
        return json({ space, editors: await listEditors('space', space.id) })
      },
      PUT: async ({ request, params }) => {
        const space = await getSpace(params.id)
        if (!space) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const editors = await listEditors('space', space.id)
        if (!canEditHuman(space, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
        const owner = isOwner(space, user.id, user.email ?? user.name)
        if (!owner && (parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined)) {
          return json({ error: 'only the owner can change sharing' }, { status: 403 })
        }
        if (owner && parsed.data.editors !== undefined) await setEditors('space', params.id, parsed.data.editors)
        const updated = await updateSpace(params.id, parsed.data)
        return json({ space: updated, editors: await listEditors('space', params.id) })
      },
      DELETE: async ({ request, params }) => {
        const space = await getSpace(params.id)
        if (!space) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const editors = await listEditors('space', space.id)
        if (!canEditHuman(space, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
        await deleteSpace(params.id)
        void logAudit({ actor: user.email ?? user.name ?? 'user', action: 'kb.space.delete', targetType: 'kb-space', targetId: params.id })
        return json({ ok: true })
      },
    },
  },
})

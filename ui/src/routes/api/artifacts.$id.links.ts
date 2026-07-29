import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { attachArtifact, detachArtifact, getArtifact, guarded } from '@/server/artifacts'
import { canRead, listEditors } from '@/server/kb-perms'

const Body = z.object({ targetType: z.string().min(1).max(40), targetId: z.string().min(1).max(200) })

// Attach / detach an artifact to/from a target (KB doc, ticket, channel, ).
// The caller must be able to read the artifact.
export const Route = createFileRoute('/api/artifacts/$id/links')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        if (!canRead(guarded(artifact), user.id, user.email ?? user.name, await listEditors('artifact', artifact.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await attachArtifact(params.id, parsed.data, user.email ?? user.name ?? 'user')
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        if (!canRead(guarded(artifact), user.id, user.email ?? user.name, await listEditors('artifact', artifact.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await detachArtifact(params.id, parsed.data)
        return json({ ok: true })
      },
    },
  },
})

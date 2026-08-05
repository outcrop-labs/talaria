import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { attachArtifact, detachArtifact, getArtifact, guarded } from '@/server/artifacts'
import { canRead, listEditors } from '@/server/kb-perms'

const Body = z.object({ targetType: z.string().min(1).max(40), targetId: z.string().min(1).max(200) })

// Attach / detach an artifact to/from a target (KB doc, ticket, channel, ).
// The caller must be able to read the artifact.
export const Route = defineApi('/api/artifacts/$id/links', {
  POST: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const artifact = await getArtifact(params.id)
    if (!artifact) return json({ error: 'not found' }, { status: 404 })
    if (!canRead(guarded(artifact), user.id, user.email ?? user.name, await listEditors('artifact', artifact.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    await attachArtifact(params.id, body, user.email ?? user.name ?? 'user')
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const artifact = await getArtifact(params.id)
    if (!artifact) return json({ error: 'not found' }, { status: 404 })
    if (!canRead(guarded(artifact), user.id, user.email ?? user.name, await listEditors('artifact', artifact.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    await detachArtifact(params.id, body)
    return json({ ok: true })
  },
})

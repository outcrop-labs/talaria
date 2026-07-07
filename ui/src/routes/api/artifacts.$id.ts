import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { deleteArtifact, getArtifact, guarded, saveArtifact } from '@/server/artifacts'
import { canEditAgent, canEditHuman, canRead, isOwner, listEditors, setEditors } from '@/server/kb-perms'

const Editor = z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1).max(200), role: z.enum(['viewer', 'editor']).default('viewer') })
const Patch = z.object({
  title: z.string().max(200).optional(),
  body: z.string().max(2_000_000).optional(),
  icon: z.string().max(16).nullish(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  editPolicy: z.enum(['owner', 'org', 'restricted']).optional(),
  editors: z.array(Editor).max(200).optional(),
})

// One artifact. Read/edit gated by its audience; sharing owner-only; agents
// (by key) only edit content when granted the Editor role.
export const Route = createFileRoute('/api/artifacts/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const editors = await listEditors('artifact', artifact.id)
        if (!canRead(guarded(artifact), user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
        return json({ artifact, editors })
      },
      PUT: async ({ request, params }) => {
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const editors = await listEditors('artifact', artifact.id)
        const g = guarded(artifact)

        let actor: string
        let owner = false
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !canEditAgent(name, editors)) return json({ error: 'forbidden' }, { status: 403 })
          actor = name
          parsed.data.visibility = undefined
          parsed.data.editPolicy = undefined
          parsed.data.editors = undefined
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!canEditHuman(g, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
          actor = user.email ?? user.name ?? 'user'
          owner = isOwner(g, user.id, user.email ?? user.name)
          const sharing = parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined
          if (!owner && sharing) return json({ error: 'only the owner can change sharing' }, { status: 403 })
        }

        if (owner && parsed.data.editors !== undefined) await setEditors('artifact', params.id, parsed.data.editors)
        const updated = await saveArtifact(params.id, parsed.data, actor)
        if (!updated) return json({ error: 'not found' }, { status: 404 })
        return json({ artifact: updated, editors: await listEditors('artifact', params.id) })
      },
      DELETE: async ({ request, params }) => {
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const editors = await listEditors('artifact', artifact.id)
        if (!canEditHuman(guarded(artifact), user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
        await deleteArtifact(params.id)
        return json({ ok: true })
      },
    },
  },
})

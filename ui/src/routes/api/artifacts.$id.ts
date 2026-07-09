import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { deleteArtifact, getArtifact, guarded, saveArtifact, setArtifactOfficial, targetsForArtifact } from '@/server/artifacts'
import { indexPlanDoc } from '@/server/plan-doc'
import { canEditAgent, canEditHuman, canRead, isOwner, listEditors, setEditors } from '@/server/kb-perms'
import { isElevatedAssistant } from '@/server/users'
import { logAudit } from '@/server/audit'

const Editor = z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1).max(200), role: z.enum(['viewer', 'editor']).default('viewer') })
const Patch = z.object({
  title: z.string().max(200).optional(),
  body: z.string().max(2_000_000).optional(),
  icon: z.string().max(16).nullish(),
  storageRef: z.string().uuid().nullish(),
  contentType: z.string().max(200).nullish(),
  folderId: z.string().uuid().nullish(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  editPolicy: z.enum(['owner', 'org', 'restricted']).optional(),
  editors: z.array(Editor).max(200).optional(),
  official: z.boolean().optional(),
})

// One artifact. Read/edit gated by its audience; sharing owner-only; agents
// (by key) only edit content when granted the Editor role.
export const Route = createFileRoute('/api/artifacts/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        const editors = await listEditors('artifact', artifact.id)
        // Agents (over MCP) read org/public artifacts + ones granted to them.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          const allowed = artifact.visibility !== 'private' || editors.some((e) => e.principalType === 'agent' && e.principalId === name)
          if (!name || !allowed) return json({ error: 'forbidden' }, { status: 403 })
          return json({ artifact, editors })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
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
          // Editor grant — or an admin-elevated assistant on any non-private artifact.
          const mayEdit = !!name && (canEditAgent(name, editors) || (artifact.visibility !== 'private' && (await isElevatedAssistant(name))))
          if (!name || !mayEdit) return json({ error: 'forbidden' }, { status: 403 })
          actor = name
          parsed.data.visibility = undefined
          parsed.data.editPolicy = undefined
          parsed.data.editors = undefined
          parsed.data.official = undefined
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!canEditHuman(g, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
          actor = user.email ?? user.name ?? 'user'
          owner = isOwner(g, user.id, user.email ?? user.name)
          const sharing = parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined
          if (!owner && sharing) return json({ error: 'only the owner can change sharing' }, { status: 403 })
        }

        if (!owner) parsed.data.official = undefined
        if (owner && parsed.data.editors !== undefined) await setEditors('artifact', params.id, parsed.data.editors)
        let updated = await saveArtifact(params.id, parsed.data, actor)
        if (!updated) return json({ error: 'not found' }, { status: 404 })
        if (parsed.data.official !== undefined && parsed.data.official !== updated.official) {
          updated = (await setArtifactOfficial(params.id, parsed.data.official, actor)) ?? updated
          void logAudit({ actor, action: parsed.data.official ? 'artifact.officialize' : 'artifact.deofficialize', targetType: 'artifact', targetId: params.id, targetLabel: updated.title })
        }
        // A plan's living document stays current in the activity brain on every
        // edit — hand edits in the side-by-side editor land here too.
        if (parsed.data.body !== undefined || parsed.data.title !== undefined) {
          const u = updated
          void targetsForArtifact(params.id)
            .then((ts) => {
              const plan = ts.find((t) => t.targetType === 'plan')
              if (plan) return indexPlanDoc(u, plan.targetId)
            })
            .catch(() => {})
        }
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

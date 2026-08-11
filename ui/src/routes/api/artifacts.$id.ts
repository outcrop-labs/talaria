import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { agentCaller } from '@/server/agent-auth'
import { deleteArtifact, getArtifact, guarded, saveArtifact, setArtifactOfficial, setArtifactRouting, targetsForArtifact } from '@/server/artifacts'
import { applyArtifactRouting } from '@/server/retrieval/artifact-routing'
import { indexPlanDoc } from '@/server/plan-doc'
import { canEditAgent, canEditHuman, canGovern, canRead, listEditors, setEditors } from '@/server/kb-perms'
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
  /** RAG routing: 'auto' | 'none' | a custom brain id. Owner-only. */
  ragRouting: z.string().max(60).optional(),
})

// One artifact. Read/edit gated by its audience; sharing owner-only; agents
// (by key) only edit content when granted the Editor role.
export const Route = defineApi('/api/artifacts/$id', {
  GET: async ({ request, params }) => {
    const artifact = await getArtifact(params.id)
    if (!artifact) return json({ error: 'not found' }, { status: 404 })
    const editors = await listEditors('artifact', artifact.id)
    // Agents (over MCP) read org/public artifacts + ones granted to them.
    const reader = await agentCaller(request)
    if (reader instanceof Response) return reader
    if (reader) {
      const name = reader.model
      const allowed = artifact.visibility !== 'private' || editors.some((e) => e.principalType === 'agent' && e.principalId === name)
      if (!allowed) return json({ error: 'forbidden' }, { status: 403 })
      return json({ artifact, editors })
    }
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    if (!canRead(guarded(artifact), user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
    return json({ artifact, editors })
  },
  PUT: async ({ request, params }) => {
    const artifact = await getArtifact(params.id)
    if (!artifact) return json({ error: 'not found' }, { status: 404 })
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    const editors = await listEditors('artifact', artifact.id)
    const g = guarded(artifact)

    let actor: string
    let owner = false
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (agent) {
      const name = agent.model
      // Editor grant — or an admin-elevated assistant on any non-private artifact.
      const mayEdit = canEditAgent(name, editors) || (artifact.visibility !== 'private' && (await isElevatedAssistant(agent)))
      if (!mayEdit) return json({ error: 'forbidden' }, { status: 403 })
      actor = name
      body.visibility = undefined
      body.editPolicy = undefined
      body.editors = undefined
      body.official = undefined
      body.ragRouting = undefined
    } else {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
      const user = gate
      if (!canEditHuman(g, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
      actor = actorOf(user)
      // `canGovern`, not `isOwner` — the same rule kb.docs.$id.ts already uses,
      // and the reason canGovern exists. An org agent's artifact is OWNERLESS
      // on purpose, so strict ownership left every workspace file an orphan
      // whose sharing literally nobody could change: the owner check could
      // never pass, and the surface offered a Share dialog that always 403'd.
      // canGovern hands those to admins and to whoever may use the agent that
      // wrote them, while human-owned artifacts stay owner-only exactly as before.
      owner = await canGovern(g, user)
      if (body.visibility === 'public' && !(await hasPerm(user, 'artifacts.publish'))) {
        return json({ error: 'no permission to publish to the web' }, { status: 403 })
      }
      const sharing = body.visibility !== undefined || body.editPolicy !== undefined || body.editors !== undefined
      if (!owner && sharing) return json({ error: 'not allowed to change sharing' }, { status: 403 })
      // Routing decides which brain retrieves the content — owner's call.
      if (!owner && body.ragRouting !== undefined) {
        return json({ error: 'only the owner can change brain routing' }, { status: 403 })
      }
    }

    if (!owner) body.official = undefined
    if (owner && body.editors !== undefined) await setEditors('artifact', params.id, body.editors)
    let updated = await saveArtifact(params.id, body, actor)
    if (!updated) return json({ error: 'not found' }, { status: 404 })
    if (body.official !== undefined && body.official !== updated.official) {
      updated = (await setArtifactOfficial(params.id, body.official, actor)) ?? updated
      void logAudit({ actor, action: body.official ? 'artifact.officialize' : 'artifact.deofficialize', targetType: 'artifact', targetId: params.id, targetLabel: updated.title })
    }
    // Routing change → re-place immediately (and validate the brain).
    if (body.ragRouting !== undefined) {
      try {
        const routed = await setArtifactRouting(params.id, body.ragRouting, actor)
        if (routed) {
          updated = routed
          void applyArtifactRouting(routed).catch(() => {})
        }
      } catch (e) {
        return json({ error: (e as Error).message }, { status: 400 })
      }
    }
    // Content edits keep the artifact's retrievable copy current: auto →
    // the plan-doc activity flow; explicit brain → re-index there.
    if (body.body !== undefined || body.title !== undefined) {
      const u = updated
      if (u.ragRouting && u.ragRouting !== 'auto') {
        void applyArtifactRouting(u).catch(() => {})
      } else {
        void targetsForArtifact(params.id)
          .then((ts) => {
            const plan = ts.find((t) => t.targetType === 'plan')
            if (plan) return indexPlanDoc(u, plan.targetId)
          })
          .catch(() => {})
      }
    }
    return json({ artifact: updated, editors: await listEditors('artifact', params.id) })
  },
  DELETE: async ({ request, params }) => {
    const artifact = await getArtifact(params.id)
    if (!artifact) return json({ error: 'not found' }, { status: 404 })
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const editors = await listEditors('artifact', artifact.id)
    if (!canEditHuman(guarded(artifact), user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
    await deleteArtifact(params.id)
    return json({ ok: true })
  },
})

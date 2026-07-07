import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { deleteDoc, getDoc, saveDoc, setOfficial } from '@/server/kb'
import { canEditAgent, canEditHuman, canRead, isOwner, listEditors, setEditors } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Editor = z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1).max(200), role: z.enum(['viewer', 'editor']).default('viewer') })
const Patch = z.object({
  title: z.string().max(200).optional(),
  body: z.string().max(500_000).optional(),
  icon: z.string().max(16).nullish(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  editPolicy: z.enum(['owner', 'org', 'restricted']).optional(),
  editors: z.array(Editor).max(200).optional(),
  parentId: z.string().uuid().nullish(),
  official: z.boolean().optional(),
})

// One KB doc. Read is gated by visibility; writes by the edit policy + editor
// grants. Sharing changes (visibility / edit policy / editors) are owner-only,
// and agents (by key) can only edit content when explicitly granted.
export const Route = createFileRoute('/api/kb/docs/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const editors = await listEditors('doc', doc.id)
        if (!canRead(doc, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
        return json({ doc, editors })
      },
      PUT: async ({ request, params }) => {
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const editors = await listEditors('doc', doc.id)

        // Authorize the writer: a granted agent (by key) or a permitted human.
        let actor: string
        let owner = false
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !canEditAgent(name, editors)) return json({ error: 'forbidden' }, { status: 403 })
          actor = name
          // Agents may only touch content — strip sharing/curation fields.
          parsed.data.visibility = undefined
          parsed.data.editPolicy = undefined
          parsed.data.editors = undefined
          parsed.data.official = undefined
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!canEditHuman(doc, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
          actor = user.email ?? user.name ?? 'user'
          owner = isOwner(doc, user.id, user.email ?? user.name)
          // Only the owner may re-share.
          if (!owner && (parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined)) {
            return json({ error: 'only the owner can change sharing' }, { status: 403 })
          }
        }

        if (owner && parsed.data.editors !== undefined) {
          await setEditors('doc', params.id, parsed.data.editors)
        }
        let updated = await saveDoc(params.id, parsed.data, actor)
        if (!updated) return json({ error: 'not found' }, { status: 404 })
        if (parsed.data.official !== undefined && parsed.data.official !== updated.official) {
          updated = (await setOfficial(params.id, parsed.data.official, actor)) ?? updated
          void logAudit({ actor, action: parsed.data.official ? 'kb.officialize' : 'kb.deofficialize', targetType: 'kb-doc', targetId: params.id, targetLabel: updated.title })
        }
        return json({ doc: updated, editors: await listEditors('doc', params.id) })
      },
      DELETE: async ({ request, params }) => {
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const editors = await listEditors('doc', doc.id)
        if (!canEditHuman(doc, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
        await deleteDoc(params.id)
        return json({ ok: true })
      },
    },
  },
})

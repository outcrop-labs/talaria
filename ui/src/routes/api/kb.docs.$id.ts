import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { deleteDoc, effectiveDocPerms, getDoc, saveDoc, setOfficial } from '@/server/kb'
import { canEditAgent, canEditHuman, canRead, isOwner, setEditors } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Editor = z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1).max(200), role: z.enum(['viewer', 'editor']).default('viewer') })
const Patch = z.object({
  title: z.string().max(200).optional(),
  body: z.string().max(500_000).optional(),
  icon: z.string().max(16).nullish(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  editPolicy: z.enum(['owner', 'org', 'restricted']).optional(),
  editors: z.array(Editor).max(200).optional(),
  permsInherited: z.boolean().optional(),
  parentId: z.string().uuid().nullish(),
  official: z.boolean().optional(),
})

// One KB doc. Read/edit are gated by the doc's EFFECTIVE audience — inherited
// from its folder unless the doc has been customized. Sharing changes are
// owner-only; agents (by key) only edit content when granted the Editor role.
export const Route = createFileRoute('/api/kb/docs/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const { perms, grants } = await effectiveDocPerms(doc)
        if (!canRead(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
        // Surface the effective visibility/policy so the UI shows what actually applies.
        return json({ doc: { ...doc, visibility: perms.visibility, editPolicy: perms.editPolicy }, editors: grants })
      },
      PUT: async ({ request, params }) => {
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const { perms, grants } = await effectiveDocPerms(doc)

        let actor: string
        let owner = false
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !canEditAgent(name, grants)) return json({ error: 'forbidden' }, { status: 403 })
          actor = name
          parsed.data.visibility = undefined
          parsed.data.editPolicy = undefined
          parsed.data.editors = undefined
          parsed.data.permsInherited = undefined
          parsed.data.official = undefined
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!canEditHuman(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
          actor = user.email ?? user.name ?? 'user'
          owner = isOwner(perms, user.id, user.email ?? user.name)
          const sharing = parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined || parsed.data.permsInherited !== undefined
          if (!owner && sharing) return json({ error: 'only the owner can change sharing' }, { status: 403 })
        }

        if (owner) {
          if (parsed.data.permsInherited === true) {
            // Reset to inherit from the folder — drop the doc's own grants.
            await setEditors('doc', params.id, [])
            parsed.data.editors = undefined
          } else if (parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined) {
            // Any explicit sharing change customizes the doc (stops inheriting).
            parsed.data.permsInherited = false
            if (parsed.data.editors !== undefined) await setEditors('doc', params.id, parsed.data.editors)
          }
        }

        let updated = await saveDoc(params.id, parsed.data, actor)
        if (!updated) return json({ error: 'not found' }, { status: 404 })
        if (parsed.data.official !== undefined && parsed.data.official !== updated.official) {
          updated = (await setOfficial(params.id, parsed.data.official, actor)) ?? updated
          void logAudit({ actor, action: parsed.data.official ? 'kb.officialize' : 'kb.deofficialize', targetType: 'kb-doc', targetId: params.id, targetLabel: updated.title })
        }
        const eff = await effectiveDocPerms(updated)
        return json({ doc: { ...updated, visibility: eff.perms.visibility, editPolicy: eff.perms.editPolicy }, editors: eff.grants })
      },
      DELETE: async ({ request, params }) => {
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const { perms, grants } = await effectiveDocPerms(doc)
        if (!canEditHuman(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
        await deleteDoc(params.id)
        return json({ ok: true })
      },
    },
  },
})

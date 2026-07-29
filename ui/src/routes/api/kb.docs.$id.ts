import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { deleteDoc, effectiveDocPerms, getDoc, saveDoc, setDocRouting, setOfficial } from '@/server/kb'
import { generateDocOkf, queueDocOkf } from '@/server/kb-okf'
import { canEditAgent, canEditHuman, canRead, canReadAgent, setEditors, canGovern } from '@/server/kb-perms'
import { isElevatedAssistant } from '@/server/users'
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
  regenerateOkf: z.boolean().optional(),
  /** RAG routing: 'auto' | 'none' | a custom brain id. Owner-only. */
  ragRouting: z.string().max(60).optional(),
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
        const { perms, grants } = await effectiveDocPerms(doc)
        // Agents (over MCP) read by effective audience: org/public, or a grant.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !canReadAgent(perms, name, grants)) return json({ error: 'forbidden' }, { status: 403 })
          return json({ doc: { ...doc, visibility: perms.visibility, editPolicy: perms.editPolicy }, editors: grants })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canRead(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
        const governs = await canGovern(perms, user)
        // Surface the effective visibility/policy so the UI shows what actually applies.
        return json({ doc: { ...doc, visibility: perms.visibility, editPolicy: perms.editPolicy, governs }, editors: grants })
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
          // Its own authored doc, an editor grant — or an admin-elevated
          // assistant on any non-private doc. Without the authorship rule an
          // agent gets 403 on the doc it JUST created (create_kb_doc grants
          // nothing) and works around it by creating duplicates.
          const mayEdit =
            !!name &&
            (doc.createdBy === name || canEditAgent(name, grants) || (perms.visibility !== 'private' && (await isElevatedAssistant(name))))
          if (!name || !mayEdit) return json({ error: 'forbidden' }, { status: 403 })
          actor = name
          parsed.data.visibility = undefined
          parsed.data.editPolicy = undefined
          parsed.data.editors = undefined
          parsed.data.permsInherited = undefined
          parsed.data.official = undefined
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!(await hasPerm(user, 'kb.edit'))) return json({ error: 'no permission to edit knowledge' }, { status: 403 })
          if (!canEditHuman(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
          // Marking OFFICIAL grounds every agent — a curation power of its own.
          if (parsed.data.official !== undefined && !(await hasPerm(user, 'kb.official'))) {
            return json({ error: 'no permission to curate official knowledge' }, { status: 403 })
          }
          actor = user.email ?? user.name ?? 'user'
          owner = await canGovern(perms, user)
          const sharing = parsed.data.visibility !== undefined || parsed.data.editPolicy !== undefined || parsed.data.editors !== undefined || parsed.data.permsInherited !== undefined
          if (!owner && sharing) return json({ error: 'only the owner can change sharing' }, { status: 403 })
          // Routing decides which brain can retrieve the doc — owner's call.
          if (!owner && parsed.data.ragRouting !== undefined) {
            return json({ error: 'only the owner can change brain routing' }, { status: 403 })
          }
        }
        if (checkAgentKey(request)) parsed.data.ragRouting = undefined

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

        if (parsed.data.ragRouting !== undefined) {
          try {
            await setDocRouting(params.id, parsed.data.ragRouting, actor)
          } catch (e) {
            return json({ error: (e as Error).message }, { status: 400 })
          }
        }
        const { regenerateOkf, ...patch } = parsed.data
        let updated = await saveDoc(params.id, patch, actor)
        if (!updated) return json({ error: 'not found' }, { status: 404 })
        if (parsed.data.official !== undefined && parsed.data.official !== updated.official) {
          updated = (await setOfficial(params.id, parsed.data.official, actor)) ?? updated
          void logAudit({ actor, action: parsed.data.official ? 'kb.officialize' : 'kb.deofficialize', targetType: 'kb-doc', targetId: params.id, targetLabel: updated.title })
          queueDocOkf(params.id) // the Librarian writes/clears this doc's OKF
        } else if (updated.official && parsed.data.body !== undefined) {
          queueDocOkf(params.id) // promoted content changed
        }
        if (regenerateOkf) {
          // Explicit regen runs against the FINAL state (post any promote/demote).
          await generateDocOkf(params.id).catch(() => {})
          updated = (await getDoc(params.id)) ?? updated
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

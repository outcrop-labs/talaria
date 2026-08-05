import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { assistantOwnerFor } from '@/server/users'
import { createDoc, getSpace, listDocs, saveDoc } from '@/server/kb'
import { canRead, canReadAgent, grantedItemIds, grantedItemIdsForAgent, listEditors } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Body = z.object({
  title: z.string().max(200).optional(),
  parentId: z.string().uuid().nullish(),
  kind: z.enum(['human', 'agent']).optional(),
  /** Initial markdown body (the MCP create_kb_doc path sets it in one shot). */
  body: z.string().max(500_000).optional(),
})

// A space's docs (tree). GET → doc metadata list. POST → new doc.
export const Route = defineApi('/api/kb/spaces/$id/docs', {
  GET: async ({ request, params }) => {
    const space = await getSpace(params.id)
    if (!space) return json({ docs: [] })
    // Agents (over MCP): gate the tree on agent space-access, then filter docs
    // by their own audience (inherited from the readable folder, or granted).
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      const name = caller.model
      if (!canReadAgent(space, name, await listEditors('space', params.id))) return json({ docs: [] })
      const grantedA = await grantedItemIdsForAgent('doc', name)
      const docsA = (await listDocs(params.id)).filter((d) => d.permsInherited || grantedA.has(d.id) || canReadAgent(d, name))
      return json({ docs: docsA })
    }
    const gate = await requirePerm(request, 'kb.edit')
    if (gate instanceof Response) return gate
    const user = gate
    // Gate the whole tree on folder access first.
    if (!canRead(space, user.id, user.email ?? user.name, await listEditors('space', params.id))) return json({ docs: [] })
    // Inherited docs are as visible as the (readable) folder, so they show.
    // Customized docs are filtered by their own audience (or an explicit grant).
    const granted = await grantedItemIds('doc', user.id)
    const docs = (await listDocs(params.id)).filter(
      (d) => d.permsInherited || granted.has(d.id) || canRead(d, user.id, user.email ?? user.name),
    )
    return json({ docs })
  },
  POST: async ({ request, params }) => {
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    // Agents (over MCP) create docs in spaces they can read. Agent docs
    // start as drafts — they never ground the org brain until a human
    // officializes them, so the write guardrail holds.
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      const name = caller.model
      const space = await getSpace(params.id)
      if (!space || !canReadAgent(space, name, await listEditors('space', params.id))) {
        return json({ error: 'forbidden' }, { status: 403 })
      }
      const doc = await createDoc({
        spaceId: params.id,
        parentId: body.parentId ?? null,
        title: body.title,
        kind: 'human',
        createdBy: name,
        // A personal assistant's doc belongs to its owner — otherwise the
        // human could never re-share what their assistant wrote for them.
        // Asked with the CALLER — owner-proxying needs proven identity.
        ownerUserId: await assistantOwnerFor(caller),
      })
      const saved = body.body ? await saveDoc(doc.id, { body: body.body }, name) : doc
      return json({ doc: saved ?? doc })
    }

    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const doc = await createDoc({
      spaceId: params.id,
      parentId: body.parentId ?? null,
      title: body.title,
      kind: body.kind,
      createdBy: user.email ?? user.name ?? 'user',
      ownerUserId: user.id,
    })
    const saved = body.body ? await saveDoc(doc.id, { body: body.body }, user.email ?? user.name ?? 'user') : doc
    void logAudit({ actor: actorOf(user), action: 'kb.doc_create', targetType: 'kb-doc', targetId: doc.id, targetLabel: doc.title })
    return json({ doc: saved ?? doc })
  },
})

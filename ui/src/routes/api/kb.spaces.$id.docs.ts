import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { personalAssistantOwners } from '@/server/users'
import { createDoc, getSpace, listDocs, saveDoc } from '@/server/kb'
import { canRead, canReadAgent, grantedItemIds, grantedItemIdsForAgent, listEditors } from '@/server/kb-perms'

const Body = z.object({
  title: z.string().max(200).optional(),
  parentId: z.string().uuid().nullish(),
  kind: z.enum(['human', 'agent']).optional(),
  /** Initial markdown body (the MCP create_kb_doc path sets it in one shot). */
  body: z.string().max(500_000).optional(),
})

// A space's docs (tree). GET → doc metadata list. POST → new doc.
export const Route = createFileRoute('/api/kb/spaces/$id/docs')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const space = await getSpace(params.id)
        if (!space) return json({ docs: [] })
        // Agents (over MCP): gate the tree on agent space-access, then filter docs
        // by their own audience (inherited from the readable folder, or granted).
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          if (!canReadAgent(space, name, await listEditors('space', params.id))) return json({ docs: [] })
          const grantedA = await grantedItemIdsForAgent('doc', name)
          const docsA = (await listDocs(params.id)).filter((d) => d.permsInherited || grantedA.has(d.id) || canReadAgent(d, name))
          return json({ docs: docsA })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'kb.edit'))) return json({ error: 'no permission to edit knowledge' }, { status: 403 })
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
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

        // Agents (over MCP) create docs in spaces they can read. Agent docs
        // start as drafts — they never ground the org brain until a human
        // officializes them, so the write guardrail holds.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          const space = await getSpace(params.id)
          if (!space || !canReadAgent(space, name, await listEditors('space', params.id))) {
            return json({ error: 'forbidden' }, { status: 403 })
          }
          const doc = await createDoc({
            spaceId: params.id,
            parentId: parsed.data.parentId ?? null,
            title: parsed.data.title,
            kind: 'agent',
            createdBy: name,
            // A personal assistant's doc belongs to its owner — otherwise the
            // human could never re-share what their assistant wrote for them.
            ownerUserId: (await personalAssistantOwners()).get(name) ?? null,
          })
          const saved = parsed.data.body ? await saveDoc(doc.id, { body: parsed.data.body }, name) : doc
          return json({ doc: saved ?? doc })
        }

        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const doc = await createDoc({
          spaceId: params.id,
          parentId: parsed.data.parentId ?? null,
          title: parsed.data.title,
          kind: parsed.data.kind,
          createdBy: user.email ?? user.name ?? 'user',
          ownerUserId: user.id,
        })
        const saved = parsed.data.body ? await saveDoc(doc.id, { body: parsed.data.body }, user.email ?? user.name ?? 'user') : doc
        return json({ doc: saved ?? doc })
      },
    },
  },
})

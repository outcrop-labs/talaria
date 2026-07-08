import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { createArtifact, guarded, listArtifacts, saveArtifact } from '@/server/artifacts'
import { canRead, grantedItemIds, grantedItemIdsForAgent, setEditors } from '@/server/kb-perms'

const Body = z.object({
  kind: z.enum(['doc', 'sheet', 'microsite', 'file']).optional(),
  title: z.string().max(200).optional(),
  body: z.string().max(2_000_000).optional(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
})

// Artifacts the caller can read. POST creates one (owned by the caller, or —
// for an agent over MCP — org-visible and editable by that agent).
export const Route = createFileRoute('/api/artifacts')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          const granted = await grantedItemIdsForAgent('artifact', name)
          // Agents see org/public artifacts + ones they've been granted.
          const artifacts = (await listArtifacts()).filter((a) => a.visibility !== 'private' || granted.has(a.id))
          return json({ artifacts })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const granted = await grantedItemIds('artifact', user.id)
        const artifacts = (await listArtifacts()).filter((a) => granted.has(a.id) || canRead(guarded(a), user.id, user.email ?? user.name))
        return json({ artifacts })
      },
      POST: async ({ request }) => {
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          const artifact = await createArtifact({ kind: parsed.data.kind, title: parsed.data.title, createdBy: name, ownerUserId: null })
          // The creating agent can keep editing it; default it org-visible so the
          // workspace can see the agent's output.
          await setEditors('artifact', artifact.id, [{ principalType: 'agent', principalId: name, role: 'editor' }])
          // Org-visible + org-editable so the workspace can pick up and manage the
          // agent's document (the agent keeps its own editor grant too).
          const updated = await saveArtifact(artifact.id, { body: parsed.data.body, visibility: parsed.data.visibility ?? 'org', editPolicy: 'org' }, name)
          return json({ artifact: updated ?? artifact })
        }

        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const artifact = await createArtifact({ kind: parsed.data.kind, title: parsed.data.title, createdBy: user.email ?? user.name ?? 'user', ownerUserId: user.id })
        const updated = parsed.data.body !== undefined ? await saveArtifact(artifact.id, { body: parsed.data.body }, user.email ?? user.name ?? 'user') : null
        return json({ artifact: updated ?? artifact })
      },
    },
  },
})

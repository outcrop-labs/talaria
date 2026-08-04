import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { assistantOwnerFor } from '@/server/users'
import { agentCategoryFolder, createArtifact, guarded, listArtifacts, namedRootFolder, saveArtifact } from '@/server/artifacts'
import { describeAgent } from '@/server/gateway'
import { canRead, grantedItemIds, grantedItemIdsForAgent, setEditors } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Body = z.object({
  kind: z.enum(['doc', 'sheet', 'microsite', 'file']).optional(),
  title: z.string().max(200).optional(),
  body: z.string().max(2_000_000).optional(),
  /** Folder by NAME (find-or-create at root) — agents filing deliberately;
   *  omitted = the agent's own cabinet. */
  folder: z.string().trim().max(120).optional(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
})

// Artifacts the caller can read. POST creates one (owned by the caller, or —
// for an agent over MCP — org-visible and editable by that agent).
export const Route = createFileRoute('/api/artifacts')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          const name = caller.model
          const granted = await grantedItemIdsForAgent('artifact', name)
          // Agents see org/public artifacts + ones they've been granted.
          const artifacts = (await listArtifacts()).filter((a) => a.visibility !== 'private' || granted.has(a.id))
          return json({ artifacts })
        }
        const gate = await requireUser(request)
        if (gate instanceof Response) return gate
        const user = gate
        const granted = await grantedItemIds('artifact', user.id)
        const artifacts = (await listArtifacts()).filter((a) => granted.has(a.id) || canRead(guarded(a), user.id, user.email ?? user.name))
        return json({ artifacts })
      },
      POST: async ({ request }) => {
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body

        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          const name = caller.model
          // WHO the agent works for decides reach: a PERSONAL assistant's
          // output belongs to its owner and stays private to them (they can
          // share it; the assistant cannot make it org-wide). A general org
          // agent's output is for the workspace — org-visible, ownerless.
          // Ask with the CALLER: attaching output to a human's account is
          // owner-proxying, so it needs a proven identity, not an asserted one.
          const paOwner = await assistantOwnerFor(caller)
          const folderId = body.folder ? await namedRootFolder(body.folder, name) : null
          const artifact = await createArtifact({
            kind: body.kind,
            title: body.title,
            createdBy: name,
            ownerUserId: paOwner,
            // Named folder (find-or-create) when the agent files deliberately;
            // its own cabinet otherwise.
            folderId: folderId ?? (await agentCategoryFolder(describeAgent(name).label, 'Documents', name)),
          })
          await setEditors('artifact', artifact.id, [{ principalType: 'agent', principalId: name, role: 'editor' }])
          const updated = await saveArtifact(
            artifact.id,
            paOwner
              ? { body: body.body, visibility: 'private', editPolicy: 'owner' }
              : { body: body.body, visibility: body.visibility ?? 'org', editPolicy: 'org' },
            name,
          )
          return json({ artifact: updated ?? artifact })
        }

        const gate = await requirePerm(request, 'artifacts.create')
        if (gate instanceof Response) return gate
        const user = gate
        const artifact = await createArtifact({ kind: body.kind, title: body.title, createdBy: user.email ?? user.name ?? 'user', ownerUserId: user.id })
        const updated = body.body !== undefined ? await saveArtifact(artifact.id, { body: body.body }, user.email ?? user.name ?? 'user') : null
        void logAudit({ actor: actorOf(user), action: 'artifact.create', targetType: 'artifact', targetId: artifact.id, targetLabel: artifact.title })
        return json({ artifact: updated ?? artifact })
      },
    },
  },
})

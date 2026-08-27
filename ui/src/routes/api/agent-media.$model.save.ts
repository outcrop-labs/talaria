import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { assistantOwnerFor, canUseAgentModel } from '@/server/users'
import { isMediaError, readAgentImage } from '@/server/agent-media'
import { agentCategoryFolder, createArtifact, createFolder, listFolders, saveArtifact } from '@/server/artifacts'
import { describeAgent } from '@/server/gateway'
import { saveUpload } from '@/server/uploads'

const Body = z.object({
  path: z.string().min(1).max(1000),
  title: z.string().trim().max(200).optional(),
  folderId: Uuid.nullish(),
  /** Folder by NAME (find-or-create) — the agent-friendly spelling. */
  folder: z.string().trim().max(120).optional(),
})

// POST { path, title?, folderId? | folder? } → copy an image out of the
// agent's container into a durable FILE artifact (uploads-backed), optionally
// straight into a folder. For science. And company meme folders. Callable by
// humans (session; any agent they may use) AND by the agent itself over the
// talaria MCP (agent key; its OWN container only). Same path/type guardrails
// as viewing the image inline.
export const Route = defineApi('/api/agent-media/$model/save', {
  POST: async ({ request, params }) => {
    let actor: string
    let ownerUserId: string | null = null
    let agentActor = false
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (agent) {
      agentActor = true
      if (agent.model !== params.model) {
        return json({ error: 'agents can only save from their own workspace' }, { status: 403 })
      }
      actor = agent.model
      // A personal assistant saves media FOR ITS OWNER — owned + private.
      // Asked with the CALLER: writing into a human's account needs a
      // proven identity, not an asserted one.
      ownerUserId = await assistantOwnerFor(agent)
    } else {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
      const user = gate
      if (!(await canUseAgentModel(user.id, user.role, params.model))) {
        return json({ error: 'forbidden' }, { status: 403 })
      }
      actor = user.email ?? user.name ?? 'user'
      ownerUserId = user.id
    }
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    const media = await readAgentImage(params.model, body.path)
    if (isMediaError(media)) return json({ error: media.error }, { status: media.status })

    // Folder by name: find-or-create (case-insensitive) — "Memes" just works.
    // With no folder given, media files under the agent's own cabinet.
    let folderId = body.folderId ?? null
    if (!folderId && body.folder) {
      const existing = (await listFolders()).find((f) => f.name.toLowerCase() === body.folder!.toLowerCase())
      folderId = existing?.id ?? (await createFolder({ name: body.folder, createdBy: actor })).id
    }
    if (!folderId) folderId = await agentCategoryFolder(describeAgent(params.model).label, 'Media', actor)

    const filename = body.path.split('/').pop() ?? 'image'
    const upload = await saveUpload({ filename, mime: media.mime, bytes: media.bytes, userId: ownerUserId })
    const created = await createArtifact({
      kind: 'file',
      title: body.title?.trim() || filename,
      createdBy: actor,
      ownerUserId,
    })
    const artifact = await saveArtifact(
      created.id,
      {
        storageRef: upload.id,
        contentType: media.mime,
        folderId,
        // ORG-agent media is for the TEAM (a private no-owner artifact
        // would be invisible to humans). A personal assistant's media
        // belongs to its owner — private, shareable by the human.
        ...(agentActor && !ownerUserId ? { visibility: 'org' as const } : {}),
      },
      actor,
    )
    return json({ artifact: artifact ?? created })
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { canUseAgentModel } from '@/server/users'
import { isMediaError, readAgentImage } from '@/server/agent-media'
import { agentCategoryFolder, createArtifact, createFolder, listFolders, saveArtifact } from '@/server/artifacts'
import { describeAgent } from '@/server/gateway'
import { saveUpload } from '@/server/uploads'

const Body = z.object({
  path: z.string().min(1).max(1000),
  title: z.string().trim().max(200).optional(),
  folderId: z.string().uuid().nullish(),
  /** Folder by NAME (find-or-create) — the agent-friendly spelling. */
  folder: z.string().trim().max(120).optional(),
})

// POST { path, title?, folderId? | folder? } → copy an image out of the
// agent's container into a durable FILE artifact (uploads-backed), optionally
// straight into a folder. For science. And company meme folders. Callable by
// humans (session; any agent they may use) AND by the agent itself over the
// talaria MCP (agent key; its OWN container only). Same path/type guardrails
// as viewing the image inline.
export const Route = createFileRoute('/api/agent-media/$model/save')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        let actor: string
        let ownerUserId: string | null = null
        let agentActor = false
        if (checkAgentKey(request)) {
          agentActor = true
          const name = agentName(request)
          if (!name || name !== params.model) {
            return json({ error: 'agents can only save from their own workspace' }, { status: 403 })
          }
          actor = name
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!(await canUseAgentModel(user.id, user.role, params.model))) {
            return json({ error: 'forbidden' }, { status: 403 })
          }
          actor = user.email ?? user.name ?? 'user'
          ownerUserId = user.id
        }
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

        const media = await readAgentImage(params.model, parsed.data.path)
        if (isMediaError(media)) return json({ error: media.error }, { status: media.status })

        // Folder by name: find-or-create (case-insensitive) — "Memes" just works.
        // With no folder given, media files under the agent's own cabinet.
        let folderId = parsed.data.folderId ?? null
        if (!folderId && parsed.data.folder) {
          const existing = (await listFolders()).find((f) => f.name.toLowerCase() === parsed.data.folder!.toLowerCase())
          folderId = existing?.id ?? (await createFolder({ name: parsed.data.folder, createdBy: actor })).id
        }
        if (!folderId) folderId = await agentCategoryFolder(describeAgent(params.model).label, 'Media', actor)

        const filename = parsed.data.path.split('/').pop() ?? 'image'
        const upload = await saveUpload({ filename, mime: media.mime, bytes: media.bytes, userId: ownerUserId })
        const created = await createArtifact({
          kind: 'file',
          title: parsed.data.title?.trim() || filename,
          createdBy: actor,
          ownerUserId,
        })
        const artifact = await saveArtifact(
          created.id,
          {
            storageRef: upload.id,
            contentType: media.mime,
            folderId,
            // Agent-saved media is for the TEAM (a private no-owner artifact
            // would be invisible to humans); people keep the private default
            // and share deliberately.
            ...(agentActor ? { visibility: 'org' as const } : {}),
          },
          actor,
        )
        return json({ artifact: artifact ?? created })
      },
    },
  },
})

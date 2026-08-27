import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { createFolder, guardedFolder, listFolders } from '@/server/artifacts'
import { canRead, grantedItemIds } from '@/server/kb-perms'

const Body = z.object({
  name: z.string().min(1).max(80),
  parentId: Uuid.nullish(),
  /** Omitted = private: a folder you make in My Files is yours until you
   *  share it. Callers wanting the old org-wide default pass it explicitly. */
  visibility: z.enum(['private', 'org', 'public']).optional(),
})

// Artifact folders. GET → the ones you can read. POST → create one you own.
export const Route = defineApi('/api/artifact-folders', {
  GET: async ({ request }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    // Folders used to be returned wholesale, which was fine while they had no
    // access of their own. Now they do, so this read is gated exactly like the
    // artifact list beside it — same canRead, same grant escape hatch.
    const granted = await grantedItemIds('artifact-folder', user.id)
    const folders = (await listFolders()).filter(
      (f) => granted.has(f.id) || canRead(guardedFolder(f), user.id, user.email ?? user.name),
    )
    return json({ folders })
  },
  POST: async ({ request }) => {
    // Folders shape the artifact tree — same perm as creating artifacts.
    const gate = await requirePerm(request, 'artifacts.create')
    if (gate instanceof Response) return gate
    const user = gate
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    return json({
      folder: await createFolder({
        name: body.name,
        parentId: body.parentId ?? null,
        createdBy: user.email ?? user.name ?? 'user',
        ownerUserId: user.id,
        visibility: body.visibility ?? 'private',
      }),
    })
  },
})

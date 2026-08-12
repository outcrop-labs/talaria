import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { deleteFolder, getFolder, guardedFolder, updateFolder } from '@/server/artifacts'
import { canEditHuman, canGovern, canRead, listEditors, setEditors } from '@/server/kb-perms'

const Patch = z.object({
  name: z.string().min(1).max(80).optional(),
  icon: z.string().max(16).nullish(),
  parentId: z.string().uuid().nullish(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  editPolicy: z.enum(['owner', 'org', 'restricted']).optional(),
  editors: z
    .array(z.object({ principalType: z.enum(['user', 'agent']), principalId: z.string().min(1), role: z.enum(['viewer', 'editor']) }))
    .optional(),
})

// One artifact folder. GET → the folder + its grants (what the Share dialog
// reads). PUT → rename / icon / reparent / re-share. DELETE → remove (its
// artifacts and child folders fall back to the root).
export const Route = defineApi('/api/artifact-folders/$id', {
  GET: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const folder = await getFolder(params.id)
    if (!folder) return json({ error: 'not found' }, { status: 404 })
    const editors = await listEditors('artifact-folder', folder.id)
    const g = guardedFolder(folder)
    if (!canRead(g, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })
    // `editors` is the key the Share dialog reads; it seeds an EDITABLE list
    // from it and PUTs that list back wholesale, so the shape is a contract.
    return json({ folder, editors })
  },
  PUT: async ({ request, params }) => {
    const gate = await requirePerm(request, 'artifacts.create')
    if (gate instanceof Response) return gate
    const user = gate
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    const folder = await getFolder(params.id)
    if (!folder) return json({ error: 'not found' }, { status: 404 })
    const editors = await listEditors('artifact-folder', folder.id)
    const g = guardedFolder(folder)
    if (!canEditHuman(g, user.id, user.email ?? user.name, editors)) return json({ error: 'forbidden' }, { status: 403 })

    const sharing = body.visibility !== undefined || body.editPolicy !== undefined || body.editors !== undefined
    if (sharing) {
      // Same governance rule as artifacts and KB docs: the owner, or an admin
      // (and agent-delegates) for an ownerless workspace folder.
      if (!(await canGovern(g, user))) return json({ error: 'not allowed to change sharing' }, { status: 403 })
      if (body.visibility === 'public' && !(await hasPerm(user, 'artifacts.publish'))) {
        return json({ error: 'no permission to publish to the web' }, { status: 403 })
      }
      if (body.editors !== undefined) await setEditors('artifact-folder', params.id, body.editors)
    }
    const updated = await updateFolder(params.id, body)
    if (!updated) return json({ error: 'invalid' }, { status: 400 })
    return json({ folder: updated })
  },
  DELETE: async ({ request, params }) => {
    const gate = await requirePerm(request, 'artifacts.create')
    if (gate instanceof Response) return gate
    const user = gate
    const folder = await getFolder(params.id)
    if (!folder) return json({ ok: true })
    // Deleting a folder scatters everything inside it to the root — a bigger
    // act than an edit, so it takes the same rights as re-sharing rather than
    // letting any org-policy editor dissolve someone else's shared folder.
    if (!(await canGovern(guardedFolder(folder), user))) return json({ error: 'forbidden' }, { status: 403 })
    await deleteFolder(params.id)
    return json({ ok: true })
  },
})

import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import {
  createSecretFolder,
  deleteSecretFolder,
  listSecretFolders,
  renameSecretFolder,
  shareSecretFolder,
} from '@/server/workspace-secrets'

// FOLDERS INSIDE THE SECRETS VIEW — and they are not the Files browser's.
//
// The first version of this filed secrets into `artifact_folders`, reasoning
// that one filing system beats two. Wrong here: a Files folder is a place for
// DOCUMENTS, it carries artifact sharing, and a secret filed into one was
// invisible from the folder it claimed to be in. People want to tidy their
// credentials where their credentials live.
//
// SHARING A FOLDER IS THE POINT, not a bonus. A set somebody is actively
// working on — the four keys the checkout rewrite needs — gets handed to a
// teammate in one gesture, and the credential added to it next week is covered
// without anybody remembering to re-share. Access is resolved at READ time as
// the union of a secret's own grants and its folder's; nothing is ever copied
// down onto rows, which would freeze membership at the moment of sharing and
// leave a stale set readable by people who should have lost it.
//
// A PERSON shared a folder can reveal everything in it. An AGENT granted a
// folder can SPEND everything in it and read none of it. Same asymmetry as
// everywhere else in this feature, for the same reason.
const Post = z.union([
  z.object({ action: z.literal('create'), name: z.string().min(1).max(60) }),
  z.object({ action: z.literal('rename'), id: Uuid, name: z.string().min(1).max(60) }),
  z.object({ action: z.literal('delete'), id: Uuid }),
  z.object({
    action: z.literal('share'),
    id: Uuid,
    on: z.boolean(),
    userId: Uuid.optional(),
    agentModel: z.string().max(120).optional(),
  }),
])
// doc: Secret folders: list / create / rename / delete. Folder membership
// doc: gates what GET /api/secrets shows.


export const Route = defineApi('/api/secrets/folders', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ folders: await listSecretFolders(user.id) })
  },

  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Post)
    if (body instanceof Response) return body
    const actor = actorOf(user)

    if (body.action === 'create') {
      const f = await createSecretFolder(body.name, user.id)
      void logAudit({ actor, action: 'secrets.folder.create', targetType: 'secret-folder', targetId: f.id, targetLabel: f.name })
      return json({ folder: f })
    }

    if (body.action === 'rename') {
      if (!(await renameSecretFolder(body.id, body.name, user.id))) return json({ error: 'not yours to rename' }, { status: 403 })
      void logAudit({ actor, action: 'secrets.folder.rename', targetType: 'secret-folder', targetId: body.id, targetLabel: body.name })
      return json({ folders: await listSecretFolders(user.id) })
    }

    if (body.action === 'delete') {
      // The credentials survive — `on delete set null` puts them back at the top
      // level. Losing four working keys because somebody tidied a label would be
      // an unforgivable way to lose them.
      if (!(await deleteSecretFolder(body.id, user.id))) return json({ error: 'not yours to delete' }, { status: 403 })
      void logAudit({ actor, action: 'secrets.folder.delete', targetType: 'secret-folder', targetId: body.id })
      return json({ folders: await listSecretFolders(user.id) })
    }

    const who = body.userId ? { userId: body.userId } : body.agentModel ? { agentModel: body.agentModel } : {}
    if (!(await shareSecretFolder(body.id, who, body.on, user.id))) return json({ error: 'not yours to share' }, { status: 403 })
    void logAudit({
      actor,
      action: `secrets.folder.${body.on ? 'share' : 'unshare'}`,
      targetType: 'secret-folder',
      targetId: body.id,
      after: who,
    })
    return json({ folders: await listSecretFolders(user.id) })
  },
})

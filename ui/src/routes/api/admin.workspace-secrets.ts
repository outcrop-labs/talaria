import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import {
  createSecretDoc,
  createSecretFolder,
  deleteSecretDoc,
  deleteSecretFolder,
  grantSecret,
  handlesHeldBy,
  listSecretDocs,
  listSecretFolders,
  moveSecretToFolder,
  revokeSecret,
  shareSecretFolder,
} from '@/server/workspace-secrets'

const Entry = z.object({
  key: z
    .string()
    .regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase letters, digits, - and _')
    .max(40),
  label: z.string().min(1).max(60),
  // Bounded generously: a PEM private key is a few thousand characters, and a
  // limit that rejects one would push somebody to paste it somewhere worse.
  value: z.string().min(1).max(20_000),
})

const Post = z.union([
  z.object({
    action: z.literal('create'),
    name: z
      .string()
      .regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase letters, digits, - and _')
      .max(40),
    title: z.string().min(1).max(80),
    entries: z.array(Entry).min(1).max(20),
    kind: z.enum(['vault', 'relay']).optional(),
    note: z.string().max(400).nullish(),
    expiresAt: z.string().max(40).nullish(),
    uses: z.number().int().min(1).max(1000).nullish(),
    grantTo: z.array(z.string().max(120)).max(50).optional(),
    /** Hosts this credential may be spent against. Empty/absent = unrestricted,
     *  which is what every secret predating the check has. */
    allowedHosts: z.array(z.string().max(253)).max(30).optional(),
  }),
  z.object({ action: z.literal('grant'), name: z.string().max(40), agentModel: z.string().min(1).max(120) }),
  z.object({ action: z.literal('revoke'), name: z.string().max(40), agentModel: z.string().min(1).max(120) }),
  z.object({ action: z.literal('delete'), name: z.string().max(40) }),
  // Folders, for grouping credentials and granting a whole set to an agent at
  // once — the same argument that made folder sharing worth building for people.
  z.object({ action: z.literal('folder-create'), name: z.string().min(1).max(60) }),
  z.object({ action: z.literal('folder-delete'), id: Uuid }),
  z.object({ action: z.literal('folder-grant'), id: Uuid, agentModel: z.string().min(1).max(120), on: z.boolean() }),
  z.object({ action: z.literal('file'), name: z.string().max(40), folderId: Uuid.nullable() }),
])

// WORKSPACE SECRETS — the credentials agents may USE without ever reading one.
//
// NOT `/api/admin/secrets`, WHICH IS A DIFFERENT THING. That route is the
// instance's own secret INVENTORY: provider keys, agent credentials, whether
// each still decrypts. This one holds credentials the workspace hands to agents.
// Two nouns, one word — worth the longer path, because an operator who conflates
// them will eventually revoke the wrong one.
//
// THE ONE RULE THIS FILE EXISTS TO HOLD: a value goes IN and never comes OUT.
// There is no GET that returns one, no echo on create, and no "reveal" verb —
// not as a permission, not for an admin, not once. An endpoint that can return a
// credential is an endpoint that will eventually return one to the wrong caller,
// and the whole arrangement downstream (`secret-vault.ts`, `resolveHandles`)
// rests on the value existing in exactly two places: the sealed column, and the
// outbound request that spends it.
//
// So the GET is deliberately dull — names, titles, entry KEYS, labels, grants,
// lifetimes. Everything a human needs to decide who may use what, and nothing
// that would help anybody use it themselves.
//
// ROTATION IS A CREATE, not an update: writing a new value over an old one under
// the same name leaves no moment where an operator can see which agents were
// using which, so replacing a credential is deleting the doc and making it
// again. That is a deliberate friction on the one operation worth being slow.
export const Route = defineApi('/api/admin/workspace-secrets', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    // `?agent=` answers the narrower question an agent's own page asks: what can
    // THIS one spend. Not derivable from the full listing — `SecretDoc.grants`
    // carries direct grants only, and a credential reaching the agent through a
    // shared folder would be missing from it.
    const agent = new URL(request.url).searchParams.get('agent')
    if (agent) return json({ held: await handlesHeldBy(agent) })
    // WORKSPACE folders — owner-less, so they belong to the org rather than to
    // whichever admin happened to make one and can outlive that account.
    return json({ secrets: await listSecretDocs(), folders: await listSecretFolders(gate.id, { workspace: true }) })
  },

  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Post)
    if (body instanceof Response) return body
    const actor = actorOf(user)

    if (body.action === 'create') {
      const doc = await createSecretDoc({
        name: body.name,
        title: body.title,
        entries: body.entries,
        ...(body.kind ? { kind: body.kind } : {}),
        note: body.note ?? null,
        createdBy: actor,
        expiresAt: body.expiresAt ?? null,
        ...(body.uses !== undefined && body.uses !== null ? { uses: body.uses } : {}),
        ...(body.grantTo ? { grantTo: body.grantTo } : {}),
        ...(body.allowedHosts ? { allowedHosts: body.allowedHosts } : {}),
      }).catch((err: unknown) => (err instanceof Error ? err : new Error(String(err))))
      if (doc instanceof Error) return json({ error: doc.message }, { status: 400 })
      // THE AUDIT LINE CARRIES KINDS, NEVER VALUES — the same rule the store
      // itself follows. `doc.entries` is keys and labels by construction.
      void logAudit({
        actor,
        action: 'secrets.create',
        targetType: 'secret',
        targetId: doc.name,
        after: { kind: doc.kind, entries: doc.entries, grants: doc.grants, uses: doc.usesRemaining, allowedHosts: doc.allowedHosts },
      })
      return json({ secret: doc })
    }

    if (body.action === 'grant') {
      await grantSecret(body.name, body.agentModel, actor)
      void logAudit({ actor, action: 'secrets.grant', targetType: 'secret', targetId: body.name, after: { agentModel: body.agentModel } })
      return json({ secrets: await listSecretDocs() })
    }

    if (body.action === 'revoke') {
      await revokeSecret(body.name, body.agentModel)
      void logAudit({ actor, action: 'secrets.revoke', targetType: 'secret', targetId: body.name, after: { agentModel: body.agentModel } })
      return json({ secrets: await listSecretDocs() })
    }

    if (body.action === 'folder-create') {
      const f = await createSecretFolder(body.name, null)
      void logAudit({ actor, action: 'secrets.folder.create', targetType: 'secret-folder', targetId: f.id, targetLabel: f.name })
      return json({ folders: await listSecretFolders(user.id, { workspace: true }) })
    }
    if (body.action === 'folder-delete') {
      // The credentials survive — `on delete set null` returns them to the top
      // level. Deleting four working keys because somebody tidied a label would
      // be an unforgivable way to lose them.
      if (!(await deleteSecretFolder(body.id, user.id, true))) return json({ error: 'no such folder' }, { status: 404 })
      void logAudit({ actor, action: 'secrets.folder.delete', targetType: 'secret-folder', targetId: body.id })
      return json({ folders: await listSecretFolders(user.id, { workspace: true }) })
    }
    if (body.action === 'folder-grant') {
      // GRANTS THE WHOLE FOLDER, now and later. A credential added to it next
      // week is covered without anybody re-granting — which is the step
      // everybody forgets, and forgetting it looks like the agent silently
      // lacking a key nobody can explain.
      if (!(await shareSecretFolder(body.id, { agentModel: body.agentModel }, body.on, user.id, true))) {
        return json({ error: 'no such folder' }, { status: 404 })
      }
      void logAudit({
        actor,
        action: `secrets.folder.${body.on ? 'grant' : 'revoke'}`,
        targetType: 'secret-folder',
        targetId: body.id,
        after: { agentModel: body.agentModel },
      })
      return json({ folders: await listSecretFolders(user.id, { workspace: true }) })
    }
    if (body.action === 'file') {
      if (!(await moveSecretToFolder(body.name, body.folderId, user.id, true))) return json({ error: 'could not file that' }, { status: 400 })
      void logAudit({ actor, action: 'secrets.move', targetType: 'secret', targetId: body.name, after: { folderId: body.folderId } })
      return json({ secrets: await listSecretDocs() })
    }

    await deleteSecretDoc(body.name)
    void logAudit({ actor, action: 'secrets.delete', targetType: 'secret', targetId: body.name })
    return json({ secrets: await listSecretDocs() })
  },
})

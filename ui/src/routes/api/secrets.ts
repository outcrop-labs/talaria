import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { createSecretDoc, deleteSecretDoc, getSecretDoc, listSecretsForUser, moveSecretToFolder } from '@/server/workspace-secrets'

// WORKING SECRETS — the ones a PERSON needs back.
//
// THREE ROUTES, THREE NOUNS, and it is worth being blunt about which is which
// because they share a table and share almost nothing else:
//
//   /api/admin/workspace-secrets  credentials an AGENT spends. Admin-only,
//                                 write-only forever, no reveal verb anywhere.
//   /api/secrets/relay            a one-shot handed to an agent mid-chat.
//   /api/secrets  (this)          a credential a person saves while building
//                                 something, shares with teammates, and READS
//                                 BACK. `revealable = true` marks exactly these.
//
// NOT ADMIN, and that is the entire reason this exists. Somebody wiring up a
// staging integration has a key their two teammates also need this week. If the
// answer is "ask an admin to add it", the real answer becomes Slack — and a key
// in a Slack thread is readable by everyone in the channel forever, with no
// record of who took it. A worse store wins on friction every time, so this one
// has to be reachable by the person with the credential in their hand.
//
// WHAT IT IS NOT: an artifact row. It wears one's clothes in the Files browser —
// folders, sharing, a title — but the VALUE never enters the artifact pipeline,
// which indexes bodies for retrieval, exports them to Google, serves downloads,
// and answers `/api/artifacts/public/$slug` with no auth at all. A credential in
// there is one visibility click from the open internet. So placement is
// artifact-shaped and storage is not.
const Entry = z.object({
  key: z
    .string()
    .regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase letters, digits, - and _')
    .max(40),
  label: z.string().min(1).max(60),
  value: z.string().min(1).max(20_000),
})

const Create = z.object({
  title: z.string().min(1).max(80),
  entries: z.array(Entry).min(1).max(20),
  note: z.string().max(400).nullish(),
  folderId: z.string().uuid().nullish(),
  /** User ids to share with at creation. The owner is always implicit. */
  readers: z.array(z.string().uuid()).max(50).optional(),
  /** Agents that may SPEND it — handle only; being shared never means readable
   *  for an agent. See the note on `grantTo` below. */
  grantTo: z.array(z.string().max(120)).max(50).optional(),
  allowedHosts: z.array(z.string().max(253)).max(30).optional(),
  expiresAt: z.string().max(40).nullish(),
})

/** A slug a person never types. Working secrets are addressed by title in the
 *  UI, so the name only has to be unique, handle-safe, and not guessable —
 *  guessable would let another agent ask for it by name and rely on the grant
 *  check as the only defence. */
const slugFor = (title: string): string =>
  `${
    title
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 24) || 'secret'
  }-${crypto.randomUUID().replace(/-/g, '').slice(0, 8)}`

export const Route = defineApi('/api/secrets', {
  // Mine, plus what has been shared with me. Keys and labels; no values — a
  // LISTING never carries one, only an explicit reveal does.
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ secrets: await listSecretsForUser(user.id) })
  },

  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Create)
    if (body instanceof Response) return body

    const doc = await createSecretDoc({
      name: slugFor(body.title),
      title: body.title,
      entries: body.entries,
      note: body.note ?? null,
      createdBy: actorOf(user),
      // THE FLAG THAT MAKES IT A DIFFERENT NOUN. Set only here — the admin route
      // never sets it, so an agent credential stays unreadable forever.
      revealable: true,
      ownerUserId: user.id,
      folderId: body.folderId ?? null,
      ...(body.readers ? { readers: body.readers } : {}),
      ...(body.grantTo ? { grantTo: body.grantTo } : {}),
      ...(body.allowedHosts ? { allowedHosts: body.allowedHosts } : {}),
      expiresAt: body.expiresAt ?? null,
    }).catch((e: unknown) => (e instanceof Error ? e : new Error(String(e))))
    if (doc instanceof Error) {
      console.error('[secrets] create failed', doc.message)
      return json({ error: 'could not save that secret — see server logs' }, { status: 500 })
    }

    void logAudit({
      actor: actorOf(user),
      action: 'secrets.save',
      targetType: 'secret',
      targetId: doc.name,
      targetLabel: doc.title,
      after: { entries: doc.entries, readers: doc.readers, grants: doc.grants, allowedHosts: doc.allowedHosts },
    })
    return json({ secret: doc })
  },

  // Move it into (or out of) a folder. Owner-only, like every other change to
  // where a credential lives.
  PATCH: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ name: z.string().max(80), folderId: z.string().uuid().nullable() }))
    if (body instanceof Response) return body
    if (!(await moveSecretToFolder(body.name, body.folderId, user.id))) return json({ error: 'not yours to move' }, { status: 403 })
    void logAudit({ actor: actorOf(user), action: 'secrets.move', targetType: 'secret', targetId: body.name, after: { folderId: body.folderId } })
    return json({ secret: await getSecretDoc(body.name) })
  },

  DELETE: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ name: z.string().max(80) }))
    if (body instanceof Response) return body

    const doc = await getSecretDoc(body.name)
    if (!doc) return json({ error: 'not found' }, { status: 404 })
    // OWNER ONLY. A reader was let in to USE the credential, not to destroy it
    // for everyone else — and an admin deleting one goes through the admin
    // route, where the act is recorded as administration rather than as the
    // owner tidying up.
    if (!doc.revealable || doc.ownerUserId !== user.id) return json({ error: 'not yours to delete' }, { status: 403 })
    await deleteSecretDoc(body.name)
    void logAudit({ actor: actorOf(user), action: 'secrets.delete', targetType: 'secret', targetId: body.name, targetLabel: doc.title })
    return json({ ok: true })
  },
})

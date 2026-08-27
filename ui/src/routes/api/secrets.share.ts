import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { getSecretDoc, grantSecret, revokeSecret, shareSecretWith, unshareSecretFrom } from '@/server/workspace-secrets'

// SHARING A WORKING SECRET — and the two audiences mean two different things.
//
//   A PERSON gets a READER grant. They can reveal it, copy it, paste it into
//     their own .env. Every look they take is audited under their name.
//   AN AGENT gets a SPEND grant. It receives the handle, can pass it to any tool
//     call, and can never see the value — not because we withhold it as a
//     policy, but because there is no code path that would hand it over.
//
// THE ASYMMETRY IS THE POINT, not an inconsistency to be tidied up later. A
// value an agent can read is a value in model context, on its way to whichever
// provider serves the next turn, and in the transcript of every turn after that.
// `secret-vault.ts` would seal it back into a placeholder on the way out, so
// "let the agent read it" does not even work — it just fails confusingly. An
// agent that needs to write a credential into a file does it through a tool
// call, which the MCP gateway substitutes into. It never needs to look.
//
// OWNER ONLY, for both. A reader was let in to USE the credential; letting them
// widen the circle turns sharing into forwarding, and the person who put the key
// in no longer knows who has it.
const Body = z.union([
  z.object({ action: z.literal('share'), name: z.string().max(80), userId: Uuid }),
  z.object({ action: z.literal('unshare'), name: z.string().max(80), userId: Uuid }),
  z.object({ action: z.literal('grant'), name: z.string().max(80), agentModel: z.string().min(1).max(120) }),
  z.object({ action: z.literal('revoke'), name: z.string().max(80), agentModel: z.string().min(1).max(120) }),
])

export const Route = defineApi('/api/secrets/share', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const actor = actorOf(user)

    if (body.action === 'share' || body.action === 'unshare') {
      const ok =
        body.action === 'share'
          ? await shareSecretWith(body.name, body.userId, user.id)
          : await unshareSecretFrom(body.name, body.userId, user.id)
      if (!ok) return json({ error: 'not yours to share' }, { status: 403 })
      void logAudit({ actor, action: `secrets.${body.action}`, targetType: 'secret', targetId: body.name, after: { userId: body.userId } })
      return json({ secret: await getSecretDoc(body.name) })
    }

    // Agent side. Ownership is checked here rather than inside `grantSecret`,
    // which the admin route also calls for workspace credentials it owns by
    // definition.
    const doc = await getSecretDoc(body.name)
    if (!doc) return json({ error: 'not found' }, { status: 404 })
    if (!doc.revealable || doc.ownerUserId !== user.id) return json({ error: 'not yours to share' }, { status: 403 })

    if (body.action === 'grant') await grantSecret(body.name, body.agentModel, actor)
    else await revokeSecret(body.name, body.agentModel)
    void logAudit({ actor, action: `secrets.${body.action}`, targetType: 'secret', targetId: body.name, after: { agentModel: body.agentModel } })
    return json({ secret: await getSecretDoc(body.name) })
  },
})

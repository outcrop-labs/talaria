import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { canUseAgentModel } from '@/server/users'
import { logAudit } from '@/server/audit'
import { mintRelay } from '@/server/workspace-secrets'

const Body = z.object({
  /** The agent this one-shot is for. Exactly one — a relay granted to a fleet is
   *  not a relay. */
  agentModel: z.string().min(1).max(120),
  /** What KIND of credential, for the human reading the row later. Never derived
   *  from the value, so it cannot leak a character of it. */
  label: z.string().min(1).max(60),
  // Bounded like the vault's: a PEM private key is a few thousand characters,
  // and a limit that rejects one pushes somebody to paste it somewhere worse.
  value: z.string().min(1).max(20_000),
  note: z.string().max(400).nullish(),
})

// HAND AN AGENT A CREDENTIAL, MID-CONVERSATION, WITHOUT PUTTING IT IN THE CHAT.
//
// The paste this exists to prevent is the ordinary one: somebody needs their
// agent to do a thing that takes a token, so they type the token into the
// message box. Once they do, it is in the transcript, in the database, in the
// prompt of every subsequent turn, and on its way to whichever provider serves
// the next reply — and no amount of guarding downstream un-sends it.
//
// So this is the SAME gesture with the value routed around the conversation: it
// arrives here on its own request, is sealed, and what goes back to the composer
// is a NAME. The message the human then sends contains that name. The value was
// never in the editor, so it is not in the transcript, and there is no path by
// which it becomes one.
//
// NOT AN ADMIN ROUTE, deliberately. `/api/admin/workspace-secrets` mints durable
// workspace credentials and is an operator's job. This is the person already
// talking to the agent, handing it something for one errand — gating it behind
// admin would mean the answer to "I need my assistant to hit this API once" is
// "file a ticket", and the observed alternative to a friction like that is
// pasting the key into the chat.
//
// WHAT BOUNDS IT INSTEAD is the same gate chat itself uses: `canUseAgentModel`.
// If you may drive this agent, you may hand it a credential for one errand — and
// only this agent, only once, only for the next hour (see RELAY_TTL_MS).
export const Route = defineApi('/api/secrets/relay', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    // The same owner-aware gate `/api/chat` applies before letting a turn
    // through — which is the point: this cannot reach an agent the caller could
    // not have talked to anyway.
    if (!(await canUseAgentModel(user.id, user.role, body.agentModel))) {
      return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
    }

    const relay = await mintRelay({
      label: body.label,
      value: body.value,
      agentModel: body.agentModel,
      createdBy: actorOf(user),
      note: body.note ?? null,
    }).catch((e: unknown) => (e instanceof Error ? e : new Error(String(e))))
    if (relay instanceof Error) {
      // Never echo the raw error to the caller: this path sits one variable away
      // from the value it was handed.
      console.error('[secrets.relay] mint failed', relay.message)
      return json({ error: 'could not mint that one-shot — see server logs' }, { status: 500 })
    }

    void logAudit({
      actor: actorOf(user),
      action: 'secrets.relay',
      targetType: 'secret',
      targetId: relay.name,
      after: { agentModel: body.agentModel, label: relay.label, expiresAt: relay.expiresAt, uses: 1 },
    })
    // THE HANDLE, AND NOTHING THAT COULD RECONSTRUCT THE VALUE. There is no read
    // path anywhere in this feature that returns one, and this is not the first.
    return json({ handle: relay.handle, name: relay.name, label: relay.label, expiresAt: relay.expiresAt })
  },
})

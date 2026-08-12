import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { revealEntry } from '@/server/workspace-secrets'

// THE ONE ROUTE IN THIS FEATURE THAT RETURNS A CREDENTIAL.
//
// Everything else — the admin panel, the relay mint, the listing above — is
// built so that a value has nowhere to come back through. This is the deliberate
// exception, and the exception is what makes the store usable by the person who
// put the credential in it. A vault nobody can read from is a vault nobody uses,
// and the thing they use instead is a Slack thread.
//
// SO THE NARROWNESS IS THE DESIGN:
//
//   ONE ENTRY, BY KEY          a bundle cannot be drained by one call, and the
//                              audit line names the credential actually looked
//                              at rather than the document it lived in.
//   ONLY `revealable = true`   an agent credential refuses here, for its owner,
//                              for an admin, for anybody. `revealEntry` checks
//                              that before it checks who is asking.
//   OWNER OR SHARED, NOT ADMIN reveal is a grant, not a role.
//   EVERY LOOK IS AUDITED      written by `revealEntry` itself, so it cannot be
//                              skipped by a second caller landing later.
//
// POST rather than GET, and that is not ceremony: a GET returning a credential
// lands in browser history, in any proxy log that records paths, and in a
// referrer header. The body keeps it out of all three.
//
// NO CACHING, said out loud in the headers below, because the default for a JSON
// response somebody fetches repeatedly is not obviously "never store this".
export const Route = defineApi('/api/secrets/reveal', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ name: z.string().max(80), key: z.string().max(40) }))
    if (body instanceof Response) return body

    const out = await revealEntry(body.name, body.key, user.id, actorOf(user))
    if (!out.ok) {
      // The refusal reason goes to the CALLER here, unlike the agent resolve
      // path: a person asking for something they own or were shared learns
      // nothing from "not shared" that they did not already know, and an opaque
      // failure on your own credential is how somebody concludes the tool is
      // broken and goes back to pasting keys into chat.
      const status = out.refusal === 'unknown' ? 404 : out.refusal === 'not-shared' || out.refusal === 'not-revealable' ? 403 : 409
      return json({ error: out.refusal }, { status })
    }

    return json(
      { value: out.value },
      {
        headers: {
          'Cache-Control': 'no-store, no-cache, must-revalidate, private',
          Pragma: 'no-cache',
          // Belt and braces on the header that would otherwise carry the path of
          // this request to wherever the page navigates next.
          'Referrer-Policy': 'no-referrer',
        },
      },
    )
  },
})

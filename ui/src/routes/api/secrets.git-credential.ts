import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody } from '@/server/api-guard'
import { requireAgent } from '@/server/agent-auth'
import { credentialForHost } from '@/server/workspace-secrets'

// THE SANDBOX'S WAY IN — where a handle could not otherwise reach.
//
// A handle substitutes at the MCP gateway, which covers every tool call an agent
// makes THROUGH Talaria. It does not cover the shell inside a workbench sandbox:
// a coding harness runs `git push` with its own bash tool, we are not in that
// path, and the handle goes out as literal text. Since pushing code is the main
// thing a workbench credential is for, "handles work everywhere except there"
// is not a mechanism anybody can build on.
//
// So git asks us. Git's credential protocol hands a helper the protocol, host
// and path on stdin and reads `username=` / `password=` back; the helper here
// forwards the host to this route over the agent's own key. Git then keeps the
// answer in process memory.
//
// WHAT THAT BUYS, precisely: the value never enters the model's context, never
// appears in command output, and is never written to disk. The model does not
// have to know a credential was involved at all — it runs `git push`, and the
// push works. That is a better outcome than a handle, not a worse one.
//
// AGENT-AUTHENTICATED, NOT SESSION-AUTHENTICATED. The caller is a process in a
// container presenting the agent's own credential, so `requireAgent` is the
// gate, and the answer is scoped to what that agent was granted. A human's
// session cannot reach this route at all — there is nothing here a person needs
// that `/api/secrets/reveal` does not already do with an audit trail.
//
// ONLY CREDENTIALS WITH AN EXPLICIT HOST ALLOWLIST are eligible; see
// `credentialForHost` for why that rule is what keeps this from being a way to
// hand every credential an agent holds to any host it can be pointed at.
export const Route = defineApi('/api/secrets/git-credential', {
  POST: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    const body = await parseBody(
      request,
      z.object({ host: z.string().min(1).max(253), protocol: z.string().max(20).optional(), path: z.string().max(400).optional() }),
    )
    if (body instanceof Response) return body

    // HTTPS ONLY. Answering for `http` would hand a live credential to a
    // cleartext connection, and a helper that does that turns one
    // misconfiguration into an interception.
    if (body.protocol && body.protocol !== 'https') {
      console.warn(`[secrets] ${caller.model} asked for a ${body.protocol} credential for ${body.host} — refused`)
      return json({ error: 'https only' }, { status: 400 })
    }

    const cred = await credentialForHost(caller.model, body.host)
    if (!cred) {
      // Reported to the OPERATOR, never elaborated to the caller: which
      // credentials exist and which hosts they cover is a map of the workspace,
      // and git only needs to know it got nothing.
      console.warn(`[secrets] ${caller.model} has no credential allowed for ${body.host}`)
      return json({ error: 'no credential for that host' }, { status: 404 })
    }

    console.warn(`[secrets] ${caller.model} spent ${cred.name} for ${body.host} via git credential helper`)
    return json(
      { username: cred.username, password: cred.password },
      { headers: { 'Cache-Control': 'no-store, no-cache, must-revalidate, private', Pragma: 'no-cache' } },
    )
  },
})

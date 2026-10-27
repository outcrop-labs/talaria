import { defineApi } from '@/server/api-route'
import { agentCaller } from '@/server/agent-auth'
import { requireUser } from '@/server/api-guard'
import { serveKit } from '@/server/sdk-kit'

// doc: GET /api/sdk/kit.tgz — the SDK kit this host build serves: declarations
// doc: for @talaria/sdk, /server and /testing, the host's app build recipe and
// doc: load probe, the shared-module list, talaria.json's schema, and the
// doc: `talaria-app` CLI (scaffold, check, build, eval), as a gzipped tar.
// doc: `x-talaria-kit-version` names the kit (the host build it came from, then
// doc: a digest of its contents), `x-talaria-sdk-version` the SDK it serves, and
// doc: `etag` its bytes (`if-none-match` answers 304; HEAD, the headers alone).
// doc: An enabled agent presenting its own credential (`x-api-key`), or a
// doc: signed-in member, may fetch it: it carries no secret, but it names this
// doc: host's build exactly. A workbench job's prepare_env installs it when the
// doc: repo has a talaria.json. See docs/sdk/outside-the-host.md.
export const Route = defineApi('/api/sdk/kit.tgz', {
  GET: async ({ request }) => {
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (!agent) {
      const user = await requireUser(request)
      if (user instanceof Response) return user
    }
    return serveKit(request)
  },
  HEAD: async ({ request }) => {
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (!agent) {
      const user = await requireUser(request)
      if (user instanceof Response) return user
    }
    return serveKit(request, true)
  },
})

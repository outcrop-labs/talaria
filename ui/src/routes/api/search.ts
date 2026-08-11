import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { DEFAULT_LIMIT } from '@/server/search'
import { searchTheWeb } from '@/server/web-search'

const Body = z.object({
  query: z.string().min(2).max(400),
  limit: z.number().int().min(1).max(25).optional(),
})

// LIVE WEB SEARCH — the endpoint behind the `web_search` MCP tool, and the one
// place an agent or a signed-in user reaches this deployment's search.
//
// IT ASKS `searchTheWeb`, NOT SEARXNG. This used to call the SearXNG client
// directly, which meant every HERMES AGENT was pinned to Talaria's own instance
// while the research harness and the fitness sweep resolved search properly —
// registry first, our engine as the floor. An org that had registered Exa got
// it everywhere EXCEPT in the agents doing their actual work. One resolver now
// answers for all of them; see `web-search.ts`.
//
// THE SAME DOOR AS `search_knowledge`, deliberately: a fleet agent authenticates
// with its agent key and a person with their session, and neither gets a
// different search. What differs from `/api/rag/search` is that this one has no
// PRINCIPAL-SCOPED CORPUS to speak of — the public web is the same web for
// everybody, so there is nothing here to scope and nothing that could leak
// between tenants. The auth check is still required: search costs upstream
// requests against engines that rate-limit by IP, and an unauthenticated
// endpoint is a free proxy for whoever finds it.
export const Route = defineApi('/api/search', {
  POST: async ({ request }) => {
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (!caller) {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
    }
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    try {
      const found = await searchTheWeb(body.query, { limit: body.limit ?? DEFAULT_LIMIT })
      // `via` rides along so a caller can say WHICH engine answered. Null is
      // Talaria's own; a named supplier is the org's registered provider.
      return json({ query: body.query, results: found.results, via: found.via })
    } catch (err) {
      // 503, NOT 500: search being unreachable is a fact about this deployment's
      // infrastructure rather than a bug in the request, and the sentence goes
      // straight into an agent's transcript — `search.ts` writes these for a
      // model to act on, so it is relayed rather than replaced.
      return json({ error: err instanceof Error ? err.message : 'search is unavailable' }, { status: 503 })
    }
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { searchForPrincipal } from '@/server/retrieval'

const Body = z.object({
  query: z.string().min(1).max(2000),
  limit: z.number().int().min(1).max(20).optional(),
  collectionIds: z.array(z.string().uuid()).max(20).optional(),
})

// Ranked retrieval across the caller's accessible collections. Works for a
// signed-in user (their bindings) OR a fleet agent (agent-key + x-agent-name →
// that agent's bindings). This is the endpoint the search_knowledge MCP tool
// calls — retrieval as function-calling.
export const Route = createFileRoute('/api/rag/search')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        let principal: { userId?: string; agentModel?: string } | null = null
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          const name = caller.model
          principal = { agentModel: name }
        } else {
          const user = await requireUser(request)
          if (user instanceof Response) return user
          principal = { userId: user.id }
        }
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        try {
          const hits = await searchForPrincipal(principal, body.query, {
            limit: body.limit,
            collectionIds: body.collectionIds,
          })
          return json({ hits })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})

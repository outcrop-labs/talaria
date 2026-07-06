import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
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
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          principal = { agentModel: name }
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          principal = { userId: user.id }
        }
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          const hits = await searchForPrincipal(principal, parsed.data.query, {
            limit: parsed.data.limit,
            collectionIds: parsed.data.collectionIds,
          })
          return json({ hits })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})

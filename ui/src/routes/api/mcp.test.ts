import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireAdmin } from '@/server/api-guard'
import { MCP_FLEET_URL, MCP_PORT } from '@/server/mcp-service'
import { probeMcp } from '@/server/mcp-probe'

const Body = z.object({
  url: z.string().url().max(300),
  // The agent slug → X-Agent-Name header (the fleet's convention), so servers
  // that scope by agent see the right identity while testing.
  agentSlug: z.string().max(80).optional(),
})

// POST → probe an MCP server's reachability + auth state (admin only; it makes
// an outbound request to an admin-supplied URL).
export const Route = createFileRoute('/api/mcp/test')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const headers: Record<string, string> = body.agentSlug ? { 'X-Agent-Name': body.agentSlug } : {}
        // The built-in toolkit endpoint: agents reach it via host.docker.internal
        // (which the host itself can't resolve on Linux) and it requires the
        // fleet key — probe locally, authenticated, without exposing the key.
        let url = body.url
        if (url === MCP_FLEET_URL()) {
          url = `http://127.0.0.1:${MCP_PORT()}/mcp`
          if (process.env.TALARIA_AGENT_KEY) headers['X-Api-Key'] = process.env.TALARIA_AGENT_KEY
        }
        return json(await probeMcp(url, headers))
      },
    },
  },
})

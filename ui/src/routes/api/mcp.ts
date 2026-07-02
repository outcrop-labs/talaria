import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listAgentMcp } from '@/server/agent-mcp'

// MCP servers per agent, read from each agent's current config version.
// Non-admins get the roster + server NAMES only — internal URLs and tool
// filters map the fleet's private attack surface, so they stay admin-only.
export const Route = createFileRoute('/api/mcp')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const agents = await listAgentMcp()
        if (user.role === 'admin') return json({ agents })
        return json({
          agents: agents.map((a) => ({
            ...a,
            servers: a.servers.map((s) => ({ name: s.name, url: '', timeout: null, extras: [] })),
          })),
        })
      },
    },
  },
})

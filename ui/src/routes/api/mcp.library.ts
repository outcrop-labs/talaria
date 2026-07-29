import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { searchMcpLibrary } from '@/server/mcp-library'

// GET ?q= → the MCP server library (the official registry, live, filtered to
// remote-capable servers). Backs the Add-server picker.
export const Route = createFileRoute('/api/mcp/library')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        const q = new URL(request.url).searchParams.get('q') ?? ''
        try {
          return json({ servers: await searchMcpLibrary(q) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})

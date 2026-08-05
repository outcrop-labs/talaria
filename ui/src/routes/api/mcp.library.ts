import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requirePerm } from '@/server/api-guard'
import { featuredMcpLibrary, searchMcpLibrary } from '@/server/mcp-library'

// GET ?q= → the MCP server library (the official registry, live, filtered to
// remote-capable servers). Backs the Add-server picker.
export const Route = defineApi('/api/mcp/library', {
  GET: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const url = new URL(request.url)
    const q = url.searchParams.get('q') ?? ''
    try {
      if (url.searchParams.get('featured') === '1') {
        return json(await featuredMcpLibrary())
      }
      return json({ servers: await searchMcpLibrary(q) })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 502 })
    }
  },
})

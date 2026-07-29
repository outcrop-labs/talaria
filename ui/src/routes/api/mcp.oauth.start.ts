import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { getMcpServer } from '@/server/mcp-registry'
import { startOauth } from '@/server/mcp-oauth'
import { instanceBaseUrl } from '@/server/instance'

// GET ?server=<id>&scope=org|me → 302 into the provider's authorization page.
// scope=org (one shared connection) needs agents.manage; scope=me connects
// the signed-in user's own account on a per-user server.
export const Route = createFileRoute('/api/mcp/oauth/start')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const url = new URL(request.url)
        const serverId = url.searchParams.get('server') ?? ''
        const scope = url.searchParams.get('scope') === 'me' ? 'me' : 'org'
        const server = await getMcpServer(serverId)
        if (!server) return json({ error: 'not found' }, { status: 404 })
        if (scope === 'org' && !(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        try {
          // A verified hosting domain gives every OAuth app ONE stable
          // callback URL, whatever origin the admin happens to browse from.
          const base = (await instanceBaseUrl()) ?? url.origin
          const authorize = await startOauth(server.id, server.url, scope === 'me' ? user.id : 'org', base)
          return new Response(null, { status: 302, headers: { location: authorize } })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})

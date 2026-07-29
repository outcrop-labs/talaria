import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasUserCredentials, listMcpServers, setUserCredentials } from '@/server/mcp-registry'
import { dropOauthTokens, hasOauthTokens } from '@/server/mcp-oauth'
import { rollAgentForUser } from '@/server/mcp-apply'
import { renderFleet } from '@/server/fleet-render'
import { logAudit } from '@/server/audit'

// Connected accounts (Settings → Connections): per-user MCP servers and
// whether YOU have connected yours. PUT { serverId, headers } connects
// (headers sealed at rest — e.g. { Authorization: "Bearer <your token>" });
// headers null disconnects. Your assistant only carries a per-user server
// once you've connected, and it acts as YOU there.
export const Route = createFileRoute('/api/me/mcp')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const servers = (await listMcpServers()).filter((s) => s.enabled && s.authMode === 'per-user')
        return json({
          servers: await Promise.all(
            servers.map(async (s) => ({
              id: s.id,
              name: s.name,
              label: s.label,
              description: s.description,
              requiredHeaders: s.requiredHeaders,
              authKind: s.oauthEnabled ? ('oauth' as const) : ('headers' as const),
              connected: s.oauthEnabled ? await hasOauthTokens(s.id, user.id) : await hasUserCredentials(s.id, user.id),
            })),
          ),
        })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({ serverId: z.string().uuid(), headers: z.record(z.string(), z.string().max(4000)).nullable() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await setUserCredentials(parsed.data.serverId, user.id, parsed.data.headers)
        if (parsed.data.headers === null) await dropOauthTokens(parsed.data.serverId, user.id)
        void logAudit({
          actor: user.email ?? user.name ?? 'user',
          action: parsed.data.headers ? 'mcp.connect' : 'mcp.disconnect',
          targetType: 'mcp-server',
          targetId: parsed.data.serverId,
        })
        void renderFleet().catch(() => {}) // config truth first…
        void rollAgentForUser(user.id).catch(() => {}) // …then the live cutover
        return json({ ok: true })
      },
    },
  },
})

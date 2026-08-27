import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
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
export const Route = defineApi('/api/me/mcp', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
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
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({ serverId: Uuid, headers: z.record(z.string(), z.string().max(4000)).nullable() }),
    )
    if (body instanceof Response) return body
    await setUserCredentials(body.serverId, user.id, body.headers)
    if (body.headers === null) await dropOauthTokens(body.serverId, user.id)
    void logAudit({
      actor: actorOf(user),
      action: body.headers ? 'mcp.connect' : 'mcp.disconnect',
      targetType: 'mcp-server',
      targetId: body.serverId,
    })
    void renderFleet().catch(() => {}) // config truth first…
    void rollAgentForUser(user.id).catch(() => {}) // …then the live cutover
    return json({ ok: true })
  },
})

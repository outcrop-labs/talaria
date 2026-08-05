import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm } from '@/server/api-guard'
import { createMcpServer, getMcpServer, listMcpServers, listAssignments, listUserAccess } from '@/server/mcp-registry'
import { ensureOauthConfig, hasOauthTokens, oauthMeta } from '@/server/mcp-oauth'
import { renderFleet } from '@/server/fleet-render'
import { logAudit } from '@/server/audit'

const Body = z.object({
  name: z.string().regex(/^[a-z0-9][a-z0-9_-]*$/, 'lowercase slug').max(60),
  label: z.string().max(120).optional(),
  description: z.string().max(500).nullish(),
  url: z.string().url().max(500),
  headers: z.record(z.string(), z.string().max(2000)).optional(),
  timeoutSecs: z.number().int().positive().max(3600).nullish(),
  authMode: z.enum(['org', 'per-user']).optional(),
  requiredHeaders: z
    .array(
      z.object({
        name: z.string().max(120),
        description: z.string().max(500).nullish(),
        isSecret: z.boolean().optional(),
        placeholder: z.string().max(200).nullish(),
      }),
    )
    .max(10)
    .optional(),
})

// The org MCP registry. GET → servers + their assignments + user access
// (admin/agents.manage view). POST → register a server. Every mutation
// re-renders the fleet so configs pick the change up (Hermes re-reads on
// mtime — no restarts).
export const Route = defineApi('/api/mcp/servers', {
  GET: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const servers = await listMcpServers()
    const detail = await Promise.all(
      servers.map(async (s) => ({
        ...s,
        headers: Object.fromEntries(Object.keys(s.headers).map((k) => [k, '•••'])), // never echo secrets
        assignments: await listAssignments(s.id),
        userAccess: await listUserAccess(s.id),
        orgConnected: s.oauthEnabled ? await hasOauthTokens(s.id, 'org') : null,
        oauthMeta: s.oauthEnabled ? await oauthMeta(s.id) : null,
      })),
    )
    return json({ servers: detail })
  },
  POST: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    try {
      let server = await createMcpServer({ ...body, createdBy: user.email ?? user.name ?? 'admin' })
      // Sniff the auth shape right away: a 401 challenge with resource
      // metadata marks the server OAuth and unlocks the Connect flow.
      const oauthCfg = await ensureOauthConfig(server.id, server.url)
      if (oauthCfg) server = (await getMcpServer(server.id)) ?? server
      void logAudit({
        actor: actorOf(user),
        action: 'mcp.server_add',
        targetType: 'mcp-server',
        targetId: server.id,
        targetLabel: server.name,
        after: { url: server.url, authMode: server.authMode },
      })
      void renderFleet().catch(() => {})
      return json({ server: { ...server, oauthMeta: server.oauthEnabled ? await oauthMeta(server.id) : null } })
    } catch (e) {
      return json({ error: (e as Error).message.includes('duplicate') ? 'that name is taken' : (e as Error).message }, { status: 400 })
    }
  },
})

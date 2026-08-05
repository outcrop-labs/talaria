import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { listAgentMcp } from '@/server/agent-mcp'
import { serversForAgent } from '@/server/mcp-registry'
import { db } from '@/server/db/pg'

// MCP servers per agent: the agent's own config version PLUS the org
// registry's assignments (rendered in at deploy — marked 'managed' here so
// the agent UI reflects what the /mcp view attached). Non-admins get the
// roster + server NAMES only — internal URLs and tool filters map the
// fleet's private attack surface, so they stay admin-only.
export const Route = defineApi('/api/mcp', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const agents = await listAgentMcp()
    const sql = await db()
    const models = (await sql`select id, model from agent_defs where enabled`) as unknown as Array<{ id: string; model: string }>
    const modelOf = new Map(models.map((m) => [m.id, m.model]))
    for (const a of agents) {
      const model = modelOf.get(a.id)
      if (!model) continue
      const have = new Set(a.servers.map((s) => s.name))
      for (const srv of await serversForAgent(model)) {
        if (have.has(srv.name)) continue
        a.servers.push({ name: srv.name, url: `gateway://${srv.name}`, timeout: srv.timeoutSecs, extras: ['managed'] })
      }
    }
    if (user.role === 'admin') return json({ agents })
    return json({
      agents: agents.map((a) => ({
        ...a,
        servers: a.servers.map((s) => ({
          name: s.name,
          url: '',
          timeout: null,
          extras: s.extras.filter((e) => e === 'built-in' || e === 'managed'),
        })),
      })),
    })
  },
})

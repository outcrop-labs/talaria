import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import {
  deleteMcpServer,
  getMcpServer,
  refreshMcpTools,
  removeAssignment,
  setAssignment,
  setUserAccess,
  updateMcpServer,
} from '@/server/mcp-registry'
import { renderFleet } from '@/server/fleet-render'
import { ensureOauthConfig, setManualOauthClient } from '@/server/mcp-oauth'
import { logAudit } from '@/server/audit'

const Patch = z.object({
  label: z.string().max(120).optional(),
  description: z.string().max(500).nullish(),
  url: z.string().url().max(500).optional(),
  headers: z.record(z.string(), z.string().max(2000)).optional(),
  timeoutSecs: z.number().int().positive().max(3600).nullish(),
  enabled: z.boolean().optional(),
  allAgents: z.boolean().optional(),
  authMode: z.enum(['org', 'per-user']).optional(),
  refreshTools: z.boolean().optional(),
  assign: z.object({ agentModel: z.string().max(200), tools: z.array(z.string().max(120)).nullable() }).optional(),
  unassign: z.string().max(200).optional(),
  userAccess: z
    .object({ userId: z.string().uuid(), allowed: z.boolean().nullable(), tools: z.array(z.string().max(120)).nullable() })
    .optional(),
  /** Pre-registered OAuth app credentials (providers without dynamic registration). */
  oauthClient: z.object({ clientId: z.string().min(1).max(200), clientSecret: z.string().max(500).nullable() }).optional(),
})

// One registry server: PUT patches config / assignment / user access / tool
// refresh in one idempotent surface; DELETE unregisters (assignments, user
// access, and connected accounts cascade). Fleet re-renders after mutations.
export const Route = createFileRoute('/api/mcp/servers/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        const server = await getMcpServer(params.id)
        if (!server) return json({ error: 'not found' }, { status: 404 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const p = parsed.data
        const actor = user.email ?? user.name ?? 'admin'

        // Self-heal: failed/aged discovery re-probes and backfills on any touch.
        await ensureOauthConfig(server.id, server.url)

        const { refreshTools, assign, unassign, userAccess, oauthClient, ...config } = p
        if (oauthClient) {
          try {
            await setManualOauthClient(
              server.id,
              server.url,
              oauthClient.clientId,
              oauthClient.clientSecret,
              `${new URL(request.url).origin}/api/mcp/oauth/callback`,
            )
            void logAudit({ actor, action: 'mcp.oauth_client', targetType: 'mcp-server', targetId: server.id, targetLabel: server.name })
          } catch (e) {
            return json({ error: (e as Error).message }, { status: 400 })
          }
        }
        if (Object.keys(config).length > 0) {
          // An omitted headers field keeps the stored secrets; sending {} clears.
          await updateMcpServer(server.id, config)
          void logAudit({ actor, action: 'mcp.server_update', targetType: 'mcp-server', targetId: server.id, targetLabel: server.name, after: { ...config, headers: config.headers ? Object.keys(config.headers) : undefined } })
        }
        if (assign) {
          await setAssignment(server.id, assign.agentModel, assign.tools)
          void logAudit({ actor, action: 'mcp.assign', targetType: 'mcp-server', targetId: server.id, targetLabel: server.name, after: assign })
        }
        if (unassign) {
          await removeAssignment(server.id, unassign)
          void logAudit({ actor, action: 'mcp.unassign', targetType: 'mcp-server', targetId: server.id, targetLabel: server.name, after: { agentModel: unassign } })
        }
        if (userAccess) {
          await setUserAccess(server.id, userAccess.userId, userAccess.allowed, userAccess.tools)
          void logAudit({ actor, action: 'mcp.user_access', targetType: 'mcp-server', targetId: server.id, targetLabel: server.name, after: userAccess })
        }
        let tools: unknown
        if (refreshTools) {
          const r = await refreshMcpTools(server.id)
          if ('error' in r) return json({ error: `tool discovery failed: ${r.error}` }, { status: 502 })
          tools = r.tools
        }
        void renderFleet().catch(() => {})
        return json({ ok: true, ...(tools !== undefined ? { tools } : {}) })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        const server = await getMcpServer(params.id)
        if (!server) return json({ error: 'not found' }, { status: 404 })
        await deleteMcpServer(server.id)
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'mcp.server_delete',
          targetType: 'mcp-server',
          targetId: server.id,
          targetLabel: server.name,
        })
        void renderFleet().catch(() => {})
        return json({ ok: true })
      },
    },
  },
})

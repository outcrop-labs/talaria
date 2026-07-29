import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser, updateSessionsForUser } from '@/server/auth/session'
import { listUsersAdmin, setAssistantElevated, setDeniedViews, setUserAgentAccess, setUserCanMintKeys, setUserRole, setAllowedManageViews } from '@/server/users'
import { logAudit } from '@/server/audit'

// Admin console API. GET → all users with roles + agent allow-lists.
// PUT { userId, role? , agentModels? } → update either. Admins only.
export const Route = createFileRoute('/api/admin/users')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ users: await listUsersAdmin() })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({
            userId: z.string().uuid(),
            role: z.enum(['admin', 'member']).optional(),
            agentModels: z.array(z.string().max(200)).max(100).optional(),
            canMintKeys: z.boolean().optional(),
            deniedViews: z.array(z.string().max(60)).max(40).optional(),
            allowedManageViews: z.array(z.string().max(60)).max(10).optional(),
            /** Promote/demote the user's personal assistant to org-wide view/edit. */
            assistantElevated: z.boolean().optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // No self-demotion — you'd lock yourself out of this page.
        if (parsed.data.role === 'member' && parsed.data.userId === user.id) {
          return json({ error: 'you cannot demote yourself' }, { status: 400 })
        }
        const actor = user.email ?? user.name ?? 'admin'
        if (parsed.data.role) {
          await setUserRole(parsed.data.userId, parsed.data.role)
          // Live sessions pick the role up immediately — no re-login dance.
          await updateSessionsForUser(parsed.data.userId, { role: parsed.data.role })
          void logAudit({ actor, action: 'user.role', targetType: 'user', targetId: parsed.data.userId, after: { role: parsed.data.role } })
          // Demotion collapses the assistant's org-wide reach with the human.
          if (parsed.data.role === 'member') await setAssistantElevated(parsed.data.userId, false)
        }
        if (parsed.data.assistantElevated !== undefined) {
          if (parsed.data.assistantElevated) {
            // Only an admin's assistant can be elevated — it inherits their standing.
            const target = (await listUsersAdmin()).find((u) => u.id === parsed.data.userId)
            const targetRole = parsed.data.role ?? target?.role
            if (targetRole !== 'admin') return json({ error: 'only an admin’s assistant can be elevated' }, { status: 400 })
            if (!target?.assistantModel) return json({ error: 'that user has no personal assistant' }, { status: 400 })
          }
          await setAssistantElevated(parsed.data.userId, parsed.data.assistantElevated)
          void logAudit({ actor, action: 'user.assistant_elevated', targetType: 'user', targetId: parsed.data.userId, after: { assistantElevated: parsed.data.assistantElevated } })
        }
        if (parsed.data.agentModels) {
          await setUserAgentAccess(parsed.data.userId, parsed.data.agentModels)
          void logAudit({ actor, action: 'user.agent_access', targetType: 'user', targetId: parsed.data.userId, after: { agentModels: parsed.data.agentModels } })
        }
        if (parsed.data.canMintKeys !== undefined) {
          await setUserCanMintKeys(parsed.data.userId, parsed.data.canMintKeys)
          void logAudit({ actor, action: 'user.can_mint_keys', targetType: 'user', targetId: parsed.data.userId, after: { canMintKeys: parsed.data.canMintKeys } })
        }
        if (parsed.data.deniedViews) {
          await setDeniedViews(parsed.data.userId, parsed.data.deniedViews)
          void logAudit({ actor, action: 'user.view_access', targetType: 'user', targetId: parsed.data.userId, after: { deniedViews: parsed.data.deniedViews } })
        }
        if (parsed.data.allowedManageViews) {
          await setAllowedManageViews(parsed.data.userId, parsed.data.allowedManageViews)
          void logAudit({ actor, action: 'user.manage_views', targetType: 'user', targetId: parsed.data.userId, after: { allowedManageViews: parsed.data.allowedManageViews } })
        }
        return json({ ok: true })
      },
    },
  },
})

import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { updateSessionsForUser } from '@/server/auth/session'
import { adminCount, listUsersAdmin, setAssistantElevated, setDeniedViews, setUserAgentAccess, setUserCanMintKeys, setUserRole, setAllowedManageViews } from '@/server/users'
import { logAudit } from '@/server/audit'

// Admin console API. GET → all users with roles + agent allow-lists.
// PUT { userId, role? , agentModels? } → update either. Admins only.
export const Route = defineApi('/api/admin/users', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({ users: await listUsersAdmin() })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        userId: Uuid,
        role: z.enum(['admin', 'member']).optional(),
        agentModels: z.array(z.string().max(200)).max(100).optional(),
        canMintKeys: z.boolean().optional(),
        deniedViews: z.array(z.string().max(60)).max(40).optional(),
        allowedManageViews: z.array(z.string().max(60)).max(10).optional(),
        /** Promote/demote the user's personal assistant to org-wide view/edit. */
        assistantElevated: z.boolean().optional(),
      }),
    )
    if (body instanceof Response) return body
    // The last admin is undemotable — that includes self-demotion, which is
    // FINE once another admin remains. Roles come from the claim and this
    // page; nothing re-grants admin after the last one is gone.
    if (body.role === 'member' && (await adminCount()) <= 1) {
      const target = (await listUsersAdmin()).find((u) => u.id === body.userId)
      if (target?.role === 'admin') {
        return json({ error: 'cannot demote the last admin' }, { status: 400 })
      }
    }
    const actor = actorOf(user)
    if (body.role) {
      await setUserRole(body.userId, body.role)
      // Live sessions pick the role up immediately — no re-login dance.
      await updateSessionsForUser(body.userId, { role: body.role })
      void logAudit({ actor, action: 'user.role', targetType: 'user', targetId: body.userId, after: { role: body.role } })
      // Demotion collapses the assistant's org-wide reach with the human.
      if (body.role === 'member') await setAssistantElevated(body.userId, false)
    }
    if (body.assistantElevated !== undefined) {
      if (body.assistantElevated) {
        // Only an admin's assistant can be elevated — it inherits their standing.
        const target = (await listUsersAdmin()).find((u) => u.id === body.userId)
        const targetRole = body.role ?? target?.role
        if (targetRole !== 'admin') return json({ error: 'only an admin’s assistant can be elevated' }, { status: 400 })
        if (!target?.assistantModel) return json({ error: 'that user has no personal assistant' }, { status: 400 })
      }
      await setAssistantElevated(body.userId, body.assistantElevated)
      void logAudit({ actor, action: 'user.assistant_elevated', targetType: 'user', targetId: body.userId, after: { assistantElevated: body.assistantElevated } })
    }
    if (body.agentModels) {
      await setUserAgentAccess(body.userId, body.agentModels)
      void logAudit({ actor, action: 'user.agent_access', targetType: 'user', targetId: body.userId, after: { agentModels: body.agentModels } })
    }
    if (body.canMintKeys !== undefined) {
      await setUserCanMintKeys(body.userId, body.canMintKeys)
      void logAudit({ actor, action: 'user.can_mint_keys', targetType: 'user', targetId: body.userId, after: { canMintKeys: body.canMintKeys } })
    }
    if (body.deniedViews) {
      await setDeniedViews(body.userId, body.deniedViews)
      void logAudit({ actor, action: 'user.view_access', targetType: 'user', targetId: body.userId, after: { deniedViews: body.deniedViews } })
    }
    if (body.allowedManageViews) {
      await setAllowedManageViews(body.userId, body.allowedManageViews)
      void logAudit({ actor, action: 'user.manage_views', targetType: 'user', targetId: body.userId, after: { allowedManageViews: body.allowedManageViews } })
    }
    return json({ ok: true })
  },
})

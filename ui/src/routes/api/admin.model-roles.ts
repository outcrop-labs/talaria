import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { getModelRoles, MODEL_ROLES, setModelRole } from '@/server/model-roles'
import { gatewayModels } from '@/server/llm-gateway'
import { logAudit } from '@/server/audit'

const ROLES = MODEL_ROLES.map((r) => r.role)

// Model Roles — which model handles each activity class. GET → the catalog of
// roles + current assignments + assignable models. PUT { role, model|null } →
// assign (null = back to auto). Admins only.
export const Route = createFileRoute('/api/admin/model-roles')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({
          roles: MODEL_ROLES,
          assignments: await getModelRoles(),
          models: (await gatewayModels()).map((m) => m.id),
        })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ role: z.enum(ROLES as [string, ...string[]]), model: z.string().max(200).nullable() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (parsed.data.model && !(await gatewayModels()).some((m) => m.id === parsed.data.model)) {
          return json({ error: 'that model is not on the gateway' }, { status: 400 })
        }
        await setModelRole(parsed.data.role as (typeof ROLES)[number], parsed.data.model)
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'model_role.assign',
          targetType: 'model-role',
          targetId: parsed.data.role,
          after: { model: parsed.data.model },
        })
        return json({ assignments: await getModelRoles() })
      },
    },
  },
})

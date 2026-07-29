import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
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
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
        return json({
          roles: MODEL_ROLES,
          assignments: await getModelRoles(),
          models: (await gatewayModels()).map((m) => m.id),
        })
      },
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(
          request,
          z.object({ role: z.enum(ROLES as [string, ...string[]]), model: z.string().max(200).nullable() }),
        )
        if (body instanceof Response) return body
        if (body.model && !(await gatewayModels()).some((m) => m.id === body.model)) {
          return json({ error: 'that model is not on the gateway' }, { status: 400 })
        }
        await setModelRole(body.role as (typeof ROLES)[number], body.model)
        void logAudit({
          actor: actorOf(user),
          action: 'model_role.assign',
          targetType: 'model-role',
          targetId: body.role,
          after: { model: body.model },
        })
        return json({ assignments: await getModelRoles() })
      },
    },
  },
})

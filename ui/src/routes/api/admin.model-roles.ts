import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { getModelRoles, MODEL_ROLES, roleAssignmentIssues, setModelRole } from '@/server/model-roles'
import { getEffortPrefs, roleSlot, setEffortPref } from '@/server/effort-prefs'
import { effortsForModel } from '@/server/model-efforts'
import { gatewayModels } from '@/server/llm-gateway'
import { logAudit } from '@/server/audit'

const ROLES = MODEL_ROLES.map((r) => r.role)

// Model Roles — which model handles each activity class. GET → the catalog of
// roles + current assignments + assignable models + fitness issues. PUT
// { role, model|null } → assign (null = back to auto). Admins only.
//
// `issues` is the audit-1.6 signal and it is ADVISORY on both verbs: a PUT that
// creates one still succeeds, and answers with the issue it just created so the
// panel can say so on the same round trip. Refusing the assignment would put
// this route in the business of overruling an admin on the strength of a probe,
// which is a call the admin is better placed to make.
export const Route = defineApi('/api/admin/model-roles', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const prefs = await getEffortPrefs()
    const efforts = Object.fromEntries(MODEL_ROLES.map((r) => [r.role, prefs[roleSlot(r.role)] ?? null]))
    return json({
      roles: MODEL_ROLES,
      assignments: await getModelRoles(),
      models: (await gatewayModels()).map((m) => m.id),
      issues: await roleAssignmentIssues(),
      // The per-role effort preference (null = the model's own default).
      efforts,
    })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        role: z.enum(ROLES as [string, ...string[]]),
        model: z.string().max(200).nullable().optional(),
        // The role's effort preference, when the admin set one. Absent =
        // leave the preference alone (a model-only save); null = clear it.
        effort: z.string().min(1).max(24).nullable().optional(),
      }),
    )
    if (body instanceof Response) return body
    if (body.model && !(await gatewayModels()).some((m) => m.id === body.model)) {
      return json({ error: 'that model is not on the gateway' }, { status: 400 })
    }
    if (body.model !== undefined) {
      await setModelRole(body.role as (typeof ROLES)[number], body.model)
      void logAudit({
        actor: actorOf(user),
        action: 'model_role.assign',
        targetType: 'model-role',
        targetId: body.role,
        after: { model: body.model },
      })
    }
    if (body.effort !== undefined) {
      // Validated against the levels the model PUBLISHES: a preference for a
      // level this model has never supported is a typo, and storing it would
      // render a picker that silently no-ops at run time. Auto (no model
      // assigned) has no ladder to validate against and no turn to ride, so
      // it is refused rather than parked.
      const target = body.model !== undefined && body.model !== null ? body.model : (await getModelRoles())[body.role as (typeof ROLES)[number]]
      if (body.effort && !target) return json({ error: 'assign a model before setting its effort' }, { status: 400 })
      if (body.effort && target && !(await effortsForModel(target)).includes(body.effort)) {
        return json({ error: `that model does not publish the "${body.effort}" effort level` }, { status: 400 })
      }
      await setEffortPref(roleSlot(body.role), body.effort)
      void logAudit({
        actor: actorOf(user),
        action: 'model_role.effort',
        targetType: 'model-role',
        targetId: body.role,
        after: { effort: body.effort },
      })
    }
    const prefs = await getEffortPrefs()
    const efforts = Object.fromEntries(MODEL_ROLES.map((r) => [r.role, prefs[roleSlot(r.role)] ?? null]))
    return json({ assignments: await getModelRoles(), issues: await roleAssignmentIssues(), efforts })
  },
})

import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin, requirePerm } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { deleteRoleTemplate, listRoleTemplates, upsertRoleTemplate } from '@/server/agent-role-templates'

// Same shapes the create dialog enforces, so a template cannot be saved in a
// state that would fail at agent-creation time.
const SLUG = z.string().min(2).max(60).regex(/^[a-z0-9]+(-[a-z0-9]+)*$/, 'lowercase-kebab')
const DEPT = z.string().min(2).max(60).regex(/^[a-z0-9]+(-[a-z0-9]+)*$/, 'lowercase-kebab')

const Body = z.object({
  slug: SLUG,
  name: z.string().min(1).max(80),
  role: z.string().min(1).max(80),
  department: DEPT,
  description: z.string().max(300).default(''),
  soul: z.string().min(1).max(20_000),
})

// Agent role templates — the business roles a new agent can start from.
// GET   → built-ins + the org's own (anyone who may create an agent needs it).
// PUT   → create or update an ORG template (admin; it seeds every future agent).
// DELETE → remove an org template; a shadowed built-in reappears.
export const Route = defineApi('/api/agent-role-templates', {
  GET: async ({ request }) => {
    const gate = await requirePerm(request, 'agents.manage')
    if (gate instanceof Response) return gate
    return json({ templates: await listRoleTemplates() })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const template = await upsertRoleTemplate(body, actorOf(user))
    void logAudit({
      actor: actorOf(user),
      action: 'agent.role_template_save',
      targetType: 'agent-role-template',
      targetId: body.slug,
      targetLabel: body.name,
    })
    return json({ template })
  },
  DELETE: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const slug = new URL(request.url).searchParams.get('slug')
    if (!slug) return json({ error: 'slug required' }, { status: 400 })
    if (!(await deleteRoleTemplate(slug))) return json({ error: 'not found' }, { status: 404 })
    void logAudit({
      actor: actorOf(user),
      action: 'agent.role_template_delete',
      targetType: 'agent-role-template',
      targetId: slug,
    })
    return json({ ok: true })
  },
})

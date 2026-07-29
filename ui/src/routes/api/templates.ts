import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { createTemplate, listTemplates } from '@/server/templates'

const Body = z.object({
  name: z.string().trim().min(1).max(120),
  kind: z.enum(['ticket', 'plan']),
  body: z.string().max(50_000).optional(),
  guidance: z.string().max(10_000).optional(),
})

// The org's template library (ticket + plan formats). GET → all (any member —
// the library grounds pickers everywhere). POST → create (any member, like
// boards/channels; the skeletons are org-shared working material, not policy).
export const Route = createFileRoute('/api/templates')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({ templates: await listTemplates() })
      },
      POST: async ({ request }) => {
        const user = await requirePerm(request, 'templates.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const template = await createTemplate({ ...body, createdBy: user.email ?? user.name ?? 'user' })
        return json({ template })
      },
    },
  },
})

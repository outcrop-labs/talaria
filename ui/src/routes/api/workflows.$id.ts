import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { deleteWorkflow, updateWorkflow } from '@/server/workflows'
import { Body } from './workflows'

const Patch = Body.partial().extend({ enabled: z.boolean().optional() })

// One task workflow: PUT patch, DELETE remove — both agents.manage.
export const Route = createFileRoute('/api/workflows/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Patch)
        if (body instanceof Response) return body
        await updateWorkflow(params.id, body)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        await deleteWorkflow(params.id)
        return json({ ok: true })
      },
    },
  },
})

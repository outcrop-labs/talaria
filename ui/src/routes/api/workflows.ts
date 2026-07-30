import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { createWorkflow, listWorkflows } from '@/server/workflows'

const Match = z.object({
  labels: z.array(z.string().min(1).max(60)).max(30).optional(),
  boards: z.array(z.string().uuid()).max(30).optional(),
  keywords: z.array(z.string().min(1).max(80)).max(30).optional(),
})
const Toolkit = z.object({ server: z.string().min(1).max(80), tools: z.array(z.string().min(1).max(120)).max(60).optional() })

export const Body = z.object({
  name: z.string().trim().min(1).max(80),
  description: z.string().max(500).optional(),
  match: Match.optional(),
  instructions: z.string().max(20_000).optional(),
  toolkits: z.array(Toolkit).max(20).optional(),
})

// Task workflows — match rules classify tickets; the payload (instructions +
// declared toolkits) rides with dispatched/picked-up work. GET → all (any
// member; they ground what agents will be told). POST → agents.manage.
export const Route = createFileRoute('/api/workflows')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({ workflows: await listWorkflows() })
      },
      POST: async ({ request }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        return json({ workflow: await createWorkflow({ ...body, createdBy: actorOf(user) }) })
      },
    },
  },
})

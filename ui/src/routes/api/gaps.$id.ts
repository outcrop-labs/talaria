import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { setGapStatus } from '@/server/gaps'

// One capability gap: PUT status (open | dismissed | resolved) — agents.manage.
// Dismissed shapes that keep recurring reopen automatically; resolved sticks.
export const Route = createFileRoute('/api/gaps/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ status: z.enum(['open', 'dismissed', 'resolved']) }))
        if (body instanceof Response) return body
        await setGapStatus(params.id, body.status)
        return json({ ok: true })
      },
    },
  },
})

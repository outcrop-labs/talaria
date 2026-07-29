import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit, getBoardAgentConfig, setBoardAgentConfig } from '@/server/boards'
import { actingUser } from '@/server/users'

const Put = z.object({
  allowAll: z.boolean().optional(),
  models: z.array(z.string().max(200)).max(100).optional(),
  add: z.array(z.string().max(200)).max(100).optional(),
  remove: z.array(z.string().max(200)).max(100).optional(),
})

// GET → { allowAll, models }. PUT → set the board's agent policy (owner/editor,
// or a personal assistant acting as its owner): either the full { allowAll,
// models } shape, or incremental { add, remove } merged onto the current list
// (the assistant-friendly spelling). Boards are restrictive by default.
export const Route = createFileRoute('/api/boards/$id/agents')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json(await getBoardAgentConfig(params.id))
      },
      PUT: async ({ request, params }) => {
        const user = await actingUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canEdit(await boardRole(user.id, params.id)) && !user.elevated) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Put)
        if (body instanceof Response) return body
        const current = await getBoardAgentConfig(params.id)
        const incremental = body.add !== undefined || body.remove !== undefined
        const models = incremental
          ? [...new Set([...current.models.filter((m) => !(body.remove ?? []).includes(m)), ...(body.add ?? [])])]
          : (body.models ?? [])
        await setBoardAgentConfig(params.id, body.allowAll ?? (incremental ? current.allowAll : false), models)
        return json(await getBoardAgentConfig(params.id))
      },
    },
  },
})

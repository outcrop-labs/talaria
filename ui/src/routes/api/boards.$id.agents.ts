import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { boardRole, canEdit, getBoardAgentConfig, setBoardAgentConfig } from '@/server/boards'
import { actingUser } from '@/server/users'

// GET → { allowAll, models }. PUT → set the board's agent policy (owner/editor,
// or a personal assistant acting as its owner): either the full { allowAll,
// models } shape, or incremental { add, remove } merged onto the current list
// (the assistant-friendly spelling). Boards are restrictive by default.
export const Route = createFileRoute('/api/boards/$id/agents')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json(await getBoardAgentConfig(params.id))
      },
      PUT: async ({ request, params }) => {
        const user = await actingUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({
            allowAll: z.boolean().optional(),
            models: z.array(z.string().max(200)).max(100).optional(),
            add: z.array(z.string().max(200)).max(100).optional(),
            remove: z.array(z.string().max(200)).max(100).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const current = await getBoardAgentConfig(params.id)
        const incremental = parsed.data.add !== undefined || parsed.data.remove !== undefined
        const models = incremental
          ? [...new Set([...current.models.filter((m) => !(parsed.data.remove ?? []).includes(m)), ...(parsed.data.add ?? [])])]
          : (parsed.data.models ?? [])
        await setBoardAgentConfig(params.id, parsed.data.allowAll ?? (incremental ? current.allowAll : false), models)
        return json(await getBoardAgentConfig(params.id))
      },
    },
  },
})

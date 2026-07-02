import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit } from '@/server/boards'
import { addDependency, getTask, removeDependency } from '@/server/tasks'

const Body = z.object({ dependsOnId: z.string().uuid() })

// POST { dependsOnId } → this ticket is blocked by another. DELETE → remove.
// Editors or board-allowed agents may add (part of triage); removal is human-only.
// The dependency target must live on the same board.
export const Route = createFileRoute('/api/tasks/$id/dependencies')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        let actor: string
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          if (!(await boardAllowsAgent(task.boardId, name))) return json({ error: 'forbidden' }, { status: 403 })
          actor = name
        } else {
          const user = await getSessionUser(request)
          if (!user || !canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
          actor = user.email ?? user.name ?? 'user'
        }
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const dep = await getTask(parsed.data.dependsOnId)
        if (!dep || dep.boardId !== task.boardId) return json({ error: 'must be a ticket on this board' }, { status: 400 })
        await addDependency(params.id, parsed.data.dependsOnId, actor)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const task = await getTask(params.id)
        if (!task || !canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await removeDependency(params.id, parsed.data.dependsOnId)
        return json({ ok: true })
      },
    },
  },
})

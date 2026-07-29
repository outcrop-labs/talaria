import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole } from '@/server/boards'
import { addWatcher, getTask, listWatchers, removeWatcher } from '@/server/tasks'

const Body = z.object({ watcher: z.string().min(1).max(200) })

// POST { watcher } → follow. DELETE { watcher } → unfollow. Board members only.
export const Route = createFileRoute('/api/tasks/$id/watchers')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const task = await getTask(params.id)
        if (!task || !(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        await addWatcher(params.id, body.watcher)
        return json({ watchers: await listWatchers(params.id) })
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const task = await getTask(params.id)
        if (!task || !(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        await removeWatcher(params.id, body.watcher)
        return json({ watchers: await listWatchers(params.id) })
      },
    },
  },
})

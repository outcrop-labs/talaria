import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { statusMeta } from '@/server/statuses'
import { addReview, getTask, updateTask, type TaskStatus } from '@/server/tasks'

const Body = z.object({ status: z.enum(['approved', 'rejected']), notes: z.string().max(20_000).optional() })

// POST /api/tasks/:id/review — the human quality gate. Approve moves the task to
// the board's done column; reject sends it back to the board's first working
// column. Board owner/editor only.
export const Route = defineApi('/api/tasks/$id/review', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const task = await getTask(params.id)
    if (!task) return json({ error: 'not found' }, { status: 404 })
    if (!canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })

    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const reviewer = user.email ?? user.name ?? 'reviewer'
    await addReview(params.id, reviewer, body.status, body.notes)
    // Boards rename and recategorize their columns, so resolve the target
    // from the BOARD — hardcoding 'done'/'in_progress' 400s human sign-off
    // on any board that renamed them. And resolve it from `statusMeta`, not
    // from `listStatuses`: the reject destination is a DESTINATION, and this
    // route was one of three siblings spelling
    // `listStatuses(...).find(s => s.category === 'active')?.key` for itself
    // — which does not exclude terminal columns, so on a board whose first
    // active column is labelled "Cancelled" (an off-board terminal key)
    // "request changes" CANCELLED the ticket. `activeKey` is picked from the
    // one `placeable` list every other destination comes from.
    // `doneKeys[0]` stays as the approve target on purpose: sign-off is the
    // one move whose destination IS terminal, it is a person's call, and the
    // membership check below catches the legacy fallback.
    const meta = await statusMeta(task.boardId)
    const back = meta.activeKey ?? meta.assignedKey
    const status = (body.status === 'approved' ? meta.doneKeys[0]! : back) as TaskStatus
    if (!meta.keys.includes(status)) {
      return json({ error: `this board has no ${body.status === 'approved' ? 'done' : 'working'} column to move the ticket into` }, { status: 400 })
    }
    const task2 = await updateTask(params.id, { status }, { kind: 'human', id: reviewer })
    return json({ task: task2 })
  },
})

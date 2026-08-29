import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { researchRole } from '@/server/research'
import { decide } from '@/server/runs/decide'

// THE EXIT FROM 'awaiting', on the run's own surface. A parked run is an
// approval (runs/decide.ts files it with the approvals machinery), and the
// research view is where the person it asked is already looking — the question
// renders in place via the projection's `awaiting` field, and this is the
// button under it. No second inbox; the run's own page IS the approval surface.
//
// THE AUTHORITY IS decide()'s, not this route's. `decide` checks the run's
// declared audience (the owner; admins for an org run) through the same
// `mayDecide` the digest and the SLA use, and this file adds only the
// visibility gate every research route starts with — a stranger probing ids
// learns nothing: no role is a 404, before decide says anything at all.
//
// THE ANSWER IS DATA: an `optionId` the step offered, plus an optional note.
// Both are clamped here and re-validated against the question the run is
// parked on RIGHT NOW inside `decide` — a stale tab answering last week's
// question is a 409, not a resume.
export const Route = defineApi('/api/research/$id/decide', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await researchRole(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })

    const body = await parseBody(request, z.object({ optionId: z.string().min(1).max(200), note: z.string().max(2000).optional() }))
    if (body instanceof Response) return body

    const res = await decide({
      runId: params.id,
      optionId: body.optionId,
      ...(body.note !== undefined && body.note.trim() ? { note: body.note } : {}),
      by: user.id,
    })
    if (res.ok) return json({ ok: true, status: res.run.state, phase: res.run.phase })

    switch (res.reason) {
      // No run under this id (or none since before the runs port). Same answer
      // as the visibility gate, for the same reason.
      case 'missing':
        return json({ error: 'not found' }, { status: 404 })
      // Answered, cancelled, or never parked. Two people racing one question is
      // the common cause and is not an error worth showing: somebody answered.
      case 'not-awaiting':
      case 'stale-key':
        return json({ error: 'this run is not waiting on a decision' }, { status: 409 })
      // Deliberately one sentence for "not in the audience" and "nobody is": a
      // route that distinguished them would say whose run this is.
      case 'forbidden':
        return json({ error: 'you cannot decide this run' }, { status: 403 })
      case 'unknown-option':
        return json({ error: 'not one of the options this run offered' }, { status: 400 })
    }
  },
})

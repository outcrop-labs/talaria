import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { deleteResearchRun, getResearchRun, researchRole } from '@/server/research'
import { assistantOwnerFor } from '@/server/users'

// GET → one run + its citation registry (owner / shared member / org runs).
// DELETE → owner/admin.
export const Route = defineApi('/api/research/$id', {
  GET: async ({ request, params }) => {
    let viewer: string | null = null
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (agent) {
      // Reading through the owner's eyes is owner-proxying — ask with the
      // CALLER so an asserted identity sees org runs only.
      viewer = await assistantOwnerFor(agent)
    } else {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
      viewer = gate.id
    }
    if (!(await researchRole(viewer, params.id))) return json({ error: 'not found' }, { status: 404 })
    const result = await getResearchRun(params.id)
    if (!result) return json({ error: 'not found' }, { status: 404 })
    return json(result)
  },
  DELETE: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const result = await getResearchRun(params.id)
    if (!result) return json({ error: 'not found' }, { status: 404 })
    if (result.run.ownerUserId !== user.id && user.role !== 'admin') {
      return json({ error: 'forbidden' }, { status: 403 })
    }
    // Cancels the run FIRST, then deletes the record — see the comment on
    // `deleteResearchRun`. Deleting alone left the driver spending on a report
    // nobody would ever open. The report artifact survives either way: deleting
    // a run clears the queue entry, not the knowledge.
    await deleteResearchRun(params.id)
    return json({ ok: true })
  },
})

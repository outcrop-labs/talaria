import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { ensureResearchConversation, researchRole } from '@/server/research'

// OPEN THE CONVERSATION FOR A RUN, creating it the first time.
//
// ON DEMAND, and that is the whole reason this is a POST rather than a field
// that always exists. Most research runs are read once and never discussed; a
// conversation row per run would turn the chat list into a list of things nobody
// said anything about. The first person to open the thread is what makes it.
//
// ACCESS IS THE RUN'S ACCESS. `researchRole` is the same check the report itself
// goes through — owner, member, or an org run with no owner — so nobody can talk
// in a room about a report they cannot read. Deliberately not a second
// permission: two answers to "may this person see this run" is how they drift.
export const Route = defineApi('/api/research/$id/conversation', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await researchRole(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })

    const conversationId = await ensureResearchConversation(params.id)
    // A run an AGENT started for the org has no human owner, so there is nobody
    // to own the conversation. The report is still readable; it just cannot be
    // talked in, and saying so beats a 500.
    if (!conversationId) {
      return json({ error: 'this run has no owner, so it has no conversation — it was started by an agent for the org' }, { status: 409 })
    }
    return json({ conversationId })
  },
})

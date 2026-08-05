import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { listAllSkills } from '@/server/agent-skills'
import { canEditSkills } from '@/server/skill-access'

// Skills across the fleet: shared + per-agent, straight from the mounts the
// agents actually read. Any member reads (the library grounds the Studio and
// what agents will be told); each owner carries canEdit for THIS user —
// admins/agents.manage everywhere, explicit user_agent_access grants (or a
// personal assistant) for that agent's own skills.
export const Route = defineApi('/api/skills', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const owners = await listAllSkills()
    const withEdit = await Promise.all(
      owners.map(async (o) => ({ ...o, canEdit: await canEditSkills(user, o.owner) })),
    )
    return json({ owners: withEdit })
  },
})

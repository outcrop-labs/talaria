import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { listAllSkills } from '@/server/agent-skills'
import { canEditSkills } from '@/server/skill-access'

// Skills across the fleet: shared + per-agent, straight from the mounts the
// agents actually read. Any member reads (the library grounds the Studio and
// what agents will be told); each owner carries canEdit for THIS user —
// admins/agents.manage everywhere, explicit user_agent_access grants (or a
// personal assistant) for that agent's own skills.
export const Route = createFileRoute('/api/skills')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const owners = await listAllSkills()
        const withEdit = await Promise.all(
          owners.map(async (o) => ({ ...o, model: undefined, canEdit: await canEditSkills(user, o.owner) })),
        )
        return json({ owners: withEdit })
      },
    },
  },
})

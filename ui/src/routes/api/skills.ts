import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireView } from '@/server/api-guard'
import { listAllSkills } from '@/server/agent-skills'

// Skills across the fleet: shared + per-agent, straight from the mounts the
// agents actually read. Fleet-wide detail: admins + Agents-view grantees.
export const Route = createFileRoute('/api/skills')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireView(request, '/agents')
        if (gate instanceof Response) return gate
        return json({ owners: await listAllSkills() })
      },
    },
  },
})

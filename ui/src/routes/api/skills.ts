import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listAllSkills } from '@/server/agent-skills'

// Skills across the fleet: shared + per-agent, straight from the mounts the
// agents actually read.
export const Route = createFileRoute('/api/skills')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ owners: await listAllSkills() })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listUsers } from '@/server/users'

// GET /api/users → everyone who has signed in (id, email, name). Powers the
// people pickers (board sharing, teams, channels). Any signed-in user.
export const Route = createFileRoute('/api/users')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!(await getSessionUser(request))) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ users: await listUsers() })
      },
    },
  },
})

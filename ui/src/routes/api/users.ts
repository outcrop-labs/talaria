import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { checkAgentKey } from '@/server/agent-auth'
import { listUsers } from '@/server/users'

// GET /api/users → everyone who has signed in (id, email, name). Powers the
// people pickers (board sharing, teams, channels). Any signed-in user — and
// agents (fleet key): they need the directory to resolve "email Priya" or
// "add Priya to the board" into an address.
export const Route = createFileRoute('/api/users')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!checkAgentKey(request) && !(await getSessionUser(request))) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ users: await listUsers() })
      },
    },
  },
})

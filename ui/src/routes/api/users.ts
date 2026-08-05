import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller } from '@/server/agent-auth'
import { listUsers } from '@/server/users'

// GET /api/users → everyone who has signed in (id, email, name). Powers the
// people pickers (board sharing, teams, channels). Any signed-in user — and
// agents (fleet key): they need the directory to resolve "email Priya" or
// "add Priya to the board" into an address.
export const Route = defineApi('/api/users', {
  GET: async ({ request }) => {
    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (!agent && !(await getSessionUser(request))) return json({ error: 'unauthorized' }, { status: 401 })
    return json({ users: await listUsers() })
  },
})

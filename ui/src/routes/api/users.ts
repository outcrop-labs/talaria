import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
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
    if (!agent) {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
    }
    return json({ users: await listUsers() })
  },
})

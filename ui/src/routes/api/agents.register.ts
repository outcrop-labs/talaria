import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody } from '@/server/api-guard'
import { checkFleetKey } from '@/server/agent-auth'
import { registerAgent } from '@/server/agents-registry'

const Body = z.object({
  name: z.string().min(1).max(200),
  role: z.string().max(80).optional(),
  capabilities: z.array(z.string()).optional(),
  framework: z.string().max(80).optional(),
})

// POST /api/agents/register — an agent registers with Talaria (MC-compatible
// contract, so the existing plugin works repointed). Agent-key auth.
export const Route = defineApi('/api/agents/register', {
  POST: async ({ request }) => {
    // Fleet-plane: the registration body names the subject, and an agent
    // registers BEFORE it has a credential of its own.
    if (!(await checkFleetKey(request))) return json({ error: 'unauthorized' }, { status: 401 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const agent = await registerAgent(body)
    return json({ agent, registered: true })
  },
})

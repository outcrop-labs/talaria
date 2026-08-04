import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
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
export const Route = createFileRoute('/api/agents/register')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        // Fleet-plane: the registration body names the subject, and an agent
        // registers BEFORE it has a credential of its own.
        if (!(await checkFleetKey(request))) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const agent = await registerAgent(parsed.data)
        return json({ agent, registered: true })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { createPersonalAgent, personalAgentFor } from '@/server/personal-agent'

// The signed-in user's personal assistant. GET → theirs (or null). POST →
// create + start one (idempotent: returns the existing one, re-enabling if
// retired). Any signed-in user may have exactly one.
export const Route = createFileRoute('/api/me/assistant')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ assistant: await personalAgentFor(user.id) })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        try {
          const assistant = await createPersonalAgent({ id: user.id, email: user.email, name: user.name })
          return json({ assistant })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})

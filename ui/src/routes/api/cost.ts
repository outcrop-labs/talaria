import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { costOverview } from '@/server/usage'

// GET /api/cost → the token ledger overview (totals, per-agent, per-day).
export const Route = createFileRoute('/api/cost')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!(await getSessionUser(request))) return json({ error: 'unauthorized' }, { status: 401 })
        return json(await costOverview())
      },
    },
  },
})

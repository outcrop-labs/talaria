import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { homeSummary } from '@/server/home'
// Side-effect import, and the only reason this line exists: server/digest.ts
// registers the daily digest and the approval-escalation jobs at module load,
// and server-entry.js starts the scheduler through whatever the ROUTE GRAPH
// pulled in (it cannot import TypeScript directly — see the handshake at the
// bottom of server/scheduler.ts). A job module nothing imports never registers,
// and the symptom is a digest that simply never arrives. This route is the
// natural anchor: the digest is built from `homeSummary`'s own queue pass.
import '@/server/digest'

// The Home/Today summary for the signed-in user.
export const Route = createFileRoute('/api/home')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json(await homeSummary(user.id, user.role))
      },
    },
  },
})

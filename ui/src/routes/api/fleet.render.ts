import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { renderFleet } from '@/server/fleet-render'

// POST → render every managed agent's config + the fleet compose + the gateway
// manifest (the bridge hot-reloads the manifest). Admin.
export const Route = createFileRoute('/api/fleet/render')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        try {
          return json({ result: await renderFleet() })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 500 })
        }
      },
    },
  },
})

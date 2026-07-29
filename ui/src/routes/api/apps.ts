import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { enabledApps } from '@/server/apps'

// The signed-in view of installed apps: ENABLED apps only, manifest data the
// client needs to draw nav items, routes, and settings tabs. Per-user view
// gating happens client-side off deniedViews (and server-side at the app API
// gateway) — this list is not secret, it is the platform's own menu.
export const Route = createFileRoute('/api/apps')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ apps: await enabledApps() })
      },
    },
  },
})

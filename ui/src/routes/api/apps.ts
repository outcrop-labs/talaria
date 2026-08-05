import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { enabledApps } from '@/server/apps'

// The signed-in view of installed apps: ENABLED apps only, manifest data the
// client needs to draw nav items, routes, and settings tabs. Per-user view
// gating happens client-side off deniedViews (and server-side at the app API
// gateway) — this list is not secret, it is the platform's own menu.
export const Route = defineApi('/api/apps', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ apps: await enabledApps() })
  },
})

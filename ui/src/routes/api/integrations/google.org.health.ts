import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { googleFail } from '@/server/google/errors'
import { probeOrgGoogleApis } from '@/server/google/api-health'

// GET /api/integrations/google/org/health → live probe of Drive / Calendar /
// Gmail with the org connection's token. Admin-only, and deliberately a
// separate route from the org status read: it makes three real Google calls
// and must only run when an admin asks for it, not on every panel load.
export const Route = defineApi('/api/integrations/google/org/health', {
  GET: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
    try {
      return json({ checkedAt: new Date().toISOString(), results: await probeOrgGoogleApis(Date.now()) })
    } catch (err) {
      return googleFail(err as Error, 'APIs')
    }
  },
})

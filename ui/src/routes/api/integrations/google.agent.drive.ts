import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { refuseLegacy, requireAgent } from '@/server/agent-auth'
import { resolveAgentGoogle } from '@/server/google/agent-google'
import { listDriveFilesWithToken } from '@/server/google/drive'
import { googleFail } from '@/server/google/errors'

// GET /api/integrations/google/agent/drive?q= → find files in the Drive the
// calling agent acts for (its owner's, or the shared org Drive). Read-only:
// finding and handing back a link. Creating files stays on
// export_to_google_doc; nothing here writes.
export const Route = defineApi('/api/integrations/google/agent/drive', {
  GET: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // A Drive listing is the owner's (or the org's) file inventory — a legacy
    // shared-key caller only ASSERTS which agent it is, so it never sees one.
    const denied = refuseLegacy(caller, 'Drive access')
    if (denied) return denied
    const google = await resolveAgentGoogle(caller.model, Date.now())
    if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
    const q = new URL(request.url).searchParams.get('q') ?? undefined
    try {
      return json({ files: await listDriveFilesWithToken(google.token, q) })
    } catch (err) {
      return googleFail(err as Error, 'Drive')
    }
  },
})

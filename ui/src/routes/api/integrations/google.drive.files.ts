import { defineApi } from '@/server/api-route'
import { requireUser } from '@/server/api-guard'
import { json } from '@/server/http'
import { listDriveFiles } from '@/server/google/drive'

// GET /api/integrations/google/drive/files?q= → browse/search the user's Drive.
export const Route = defineApi('/api/integrations/google/drive/files', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const q = new URL(request.url).searchParams.get('q') ?? undefined
    try {
      const files = await listDriveFiles(user.id, Date.now(), q)
      return json({ files })
    } catch (err) {
      const e = err as Error
      if (e.name === 'GoogleNotConnected') {
        return json({ error: 'not_connected', message: 'Connect a Google account first.' }, { status: 409 })
      }
      if (/insufficient|ACCESS_TOKEN_SCOPE/i.test(e.message)) {
        return json({ error: 'reconnect_needed', message: 'Reconnect Google to grant Drive read access.' }, { status: 409 })
      }
      if (import.meta.env.DEV) console.error('[drive/files] failed:', e)
      return json({ error: 'drive_error', message: 'Could not reach Google Drive.' }, { status: 502 })
    }
  },
})

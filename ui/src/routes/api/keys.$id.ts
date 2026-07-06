import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { revokeKey } from '@/server/llm-keys'

// DELETE → revoke one of MY keys (immediate; the hash stays for audit).
export const Route = createFileRoute('/api/keys/$id')({
  server: {
    handlers: {
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        await revokeKey(user.id, params.id)
        return json({ ok: true })
      },
    },
  },
})

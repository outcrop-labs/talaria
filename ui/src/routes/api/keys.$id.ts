import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { revokeKey } from '@/server/llm-keys'
import { logAudit } from '@/server/audit'

// DELETE → revoke one of MY keys (immediate; the hash stays for audit).
export const Route = createFileRoute('/api/keys/$id')({
  server: {
    handlers: {
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        await revokeKey(user.id, params.id)
        void logAudit({
          actor: user.email ?? user.name ?? 'user',
          action: 'key.revoke',
          targetType: 'llm-key',
          targetId: params.id,
        })
        return json({ ok: true })
      },
    },
  },
})

import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { actorOf, requireUser } from '@/server/api-guard'
import { revokeKey } from '@/server/llm-keys'
import { logAudit } from '@/server/audit'

// DELETE → revoke one of MY keys (immediate; the hash stays for audit).
export const Route = defineApi('/api/keys/$id', {
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    await revokeKey(user.id, params.id)
    void logAudit({
      actor: actorOf(user),
      action: 'key.revoke',
      targetType: 'llm-key',
      targetId: params.id,
    })
    return json({ ok: true })
  },
})

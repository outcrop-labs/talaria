import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { canMintKeys, listKeys, mintKey } from '@/server/llm-keys'
import { logAudit } from '@/server/audit'

const Body = z.object({ name: z.string().min(1).max(60) })

// Personal API keys for the Talaria LLM gateway. GET → my keys (+ whether I
// may mint). POST → mint one; the secret is in THIS response only.
export const Route = defineApi('/api/keys', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ keys: await listKeys(user.id), canMint: await canMintKeys(user.id, user.role) })
  },
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await canMintKeys(user.id, user.role))) {
      return json({ error: 'API keys are not enabled for your account — ask an admin' }, { status: 403 })
    }
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const { key, secret } = await mintKey(user.id, body.name)
    void logAudit({
      actor: actorOf(user),
      action: 'key.mint',
      targetType: 'llm-key',
      targetId: key.id,
      targetLabel: body.name,
    })
    return json({ key, secret })
  },
})

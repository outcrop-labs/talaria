import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { revokeKey, setKeyPolicy } from '@/server/llm-keys'
import { logAudit } from '@/server/audit'

// DELETE → revoke one of MY keys (immediate; the hash stays for audit).
// PUT → set MY key's policy (#265): a spend cap (tokens/$ over the org budget
// window) and a per-minute request ceiling. All null/0 = unlimited. The spend
// cap cannot out-spend an admin ceiling — checkBudget min-merges — but it is
// the owner's to raise or remove, and the denial message says so.
const Policy = z.object({
  spendCapTokens: z.number().int().min(0).max(1e15).nullish(),
  spendCapUsd: z.number().min(0).max(1e9).nullish(),
  rateLimitPerMinute: z.number().int().min(0).max(10_000).nullish(),
})

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
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Policy)
    if (body instanceof Response) return body
    const policy = {
      spendCapTokens: body.spendCapTokens ?? null,
      spendCapUsd: body.spendCapUsd ?? null,
      rateLimitPerMinute: body.rateLimitPerMinute ?? null,
    }
    if (!(await setKeyPolicy(user.id, params.id, policy))) {
      return json({ error: 'no such key' }, { status: 404 })
    }
    void logAudit({
      actor: actorOf(user),
      action: 'key.policy',
      targetType: 'llm-key',
      targetId: params.id,
      after: policy,
    })
    return json({ ok: true, policy })
  },
})

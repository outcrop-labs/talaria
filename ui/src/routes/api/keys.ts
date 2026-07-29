import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { canMintKeys, listKeys, mintKey } from '@/server/llm-keys'
import { logAudit } from '@/server/audit'

const Body = z.object({ name: z.string().min(1).max(60) })

// Personal API keys for the Talaria LLM gateway. GET → my keys (+ whether I
// may mint). POST → mint one; the secret is in THIS response only.
export const Route = createFileRoute('/api/keys')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ keys: await listKeys(user.id), canMint: await canMintKeys(user.id, user.role) })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canMintKeys(user.id, user.role))) {
          return json({ error: 'API keys are not enabled for your account — ask an admin' }, { status: 403 })
        }
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const { key, secret } = await mintKey(user.id, parsed.data.name)
        void logAudit({
          actor: user.email ?? user.name ?? 'user',
          action: 'key.mint',
          targetType: 'llm-key',
          targetId: key.id,
          targetLabel: parsed.data.name,
        })
        return json({ key, secret })
      },
    },
  },
})

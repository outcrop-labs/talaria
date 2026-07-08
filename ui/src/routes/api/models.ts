import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { gatewayModels } from '@/server/llm-gateway'
import { copilotModelFor } from '@/server/copilot'

// The gateway model catalog for signed-in users (the /api/llm/v1/models twin
// without an API key) — powers the preferred-model picker. Also says which
// model the caller's copilot would use right now (pref → default fallback).
export const Route = createFileRoute('/api/models')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ models: await gatewayModels(), effective: await copilotModelFor(user.id) })
      },
    },
  },
})

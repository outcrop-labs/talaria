import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { gatewayModelsFor } from '@/server/model-access'
import { modelInfo } from '@/server/model-info'
import { museModelFor } from '@/server/muse'

// The gateway model catalog for signed-in users (the /api/llm/v1/models twin
// without an API key) — powers the preferred-model picker. Role-filtered:
// members see only what the admin allowlist permits; admins see everything.
// Each model carries a pretty label + a "what it's good at" blurb when the
// public catalog knows it. Also says which model the caller's muse would use.
export const Route = createFileRoute('/api/models')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const models = await Promise.all(
          (await gatewayModelsFor(user.role)).map(async (m) => {
            const info = await modelInfo(m.qualified ? m.id.slice(m.id.indexOf('/') + 1) : m.id)
            return info ? { ...m, label: info.label, blurb: info.blurb } : m
          }),
        )
        return json({ models, effective: await museModelFor(user.id) })
      },
    },
  },
})

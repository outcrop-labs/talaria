import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { artifactsForTarget, guarded } from '@/server/artifacts'
import { canRead } from '@/server/kb-perms'

// Artifacts attached to a given target (e.g. a KB doc), filtered to the ones
// the caller can read.  GET /api/artifacts/for?targetType=kb-doc&targetId=<id>
export const Route = createFileRoute('/api/artifacts/for')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const q = new URL(request.url).searchParams
        const targetType = q.get('targetType')
        const targetId = q.get('targetId')
        if (!targetType || !targetId) return json({ artifacts: [] })
        const artifacts = (await artifactsForTarget(targetType, targetId)).filter((a) => canRead(guarded(a), user.id, user.email ?? user.name))
        return json({ artifacts })
      },
    },
  },
})

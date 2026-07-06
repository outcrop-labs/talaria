import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { activityFeed, type ActivityKind } from '@/server/activity-feed'

const KINDS = new Set(['ticket', 'channel', 'fleet', 'audit'])

// The merged workspace activity feed, scoped to the requesting user.
export const Route = createFileRoute('/api/activity')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const kinds = (new URL(request.url).searchParams.get('kinds') ?? '')
          .split(',')
          .filter((k) => KINDS.has(k)) as ActivityKind[]
        return json({ events: await activityFeed(user.id, kinds, 80, user.role === 'admin') })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { activityFeed, type ActivityKind } from '@/server/activity-feed'

const KINDS = new Set(['ticket', 'channel', 'fleet', 'audit'])

// The merged workspace activity feed, scoped to the requesting user.
export const Route = createFileRoute('/api/activity')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const kinds = (new URL(request.url).searchParams.get('kinds') ?? '')
          .split(',')
          .filter((k) => KINDS.has(k)) as ActivityKind[]
        return json({ events: await activityFeed(user.id, kinds, 80, user.role === 'admin') })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { deniedViews } from '@/server/users'
import { userPermissions } from '@/server/permissions'

// GET /api/auth/session → the current user + their denied views + effective
// permissions (read from the DB each time, so an admin's access change
// applies without re-login).
export const Route = createFileRoute('/api/auth/session')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        const [denied, perms] = user
          ? await Promise.all([deniedViews(user.id, user.role), userPermissions(user.id, user.role)])
          : [[], []]
        return json({ user, deniedViews: denied, perms })
      },
    },
  },
})

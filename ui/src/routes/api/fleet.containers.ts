import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { containerStatus } from '@/server/fleet-docker'
import { db } from '@/server/db/pg'

// GET → container reality per agent (managed + legacy), admin.
export const Route = createFileRoute('/api/fleet/containers')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const sql = await db()
        const rows = (await sql`select department from agent_defs where enabled order by slug`) as unknown as Array<{
          department: string
        }>
        return json({ containers: await containerStatus(rows.map((r) => r.department)) })
      },
    },
  },
})

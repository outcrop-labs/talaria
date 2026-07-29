import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireAdmin } from '@/server/api-guard'
import { containerStatus } from '@/server/fleet-docker'
import { db } from '@/server/db/pg'

// GET → container reality per agent (the managed service), admin.
export const Route = createFileRoute('/api/fleet/containers')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const sql = await db()
        const rows = (await sql`select department from agent_defs where enabled order by slug`) as unknown as Array<{
          department: string
        }>
        return json({ containers: await containerStatus(rows.map((r) => r.department)) })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { channelRole } from '@/server/channels'
import { concludeRelay } from '@/server/comms-decay'
import { db } from '@/server/db/pg'

// POST → conclude a Relay: summarize what was decided (posted as the final
// message + indexed for retrieval), then archive it. Members only; relays only.
export const Route = createFileRoute('/api/channels/$id/conclude')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const sql = await db()
        const rows = (await sql`select name, kind from channels where id = ${params.id} and archived_at is null`) as unknown as Array<{
          name: string
          kind: string
        }>
        if (!rows[0]) return json({ error: 'not found' }, { status: 404 })
        if (rows[0].kind !== 'group') return json({ error: 'only relays conclude — channels persist' }, { status: 400 })
        try {
          return json({ summary: await concludeRelay(params.id, user.id, rows[0].name) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})

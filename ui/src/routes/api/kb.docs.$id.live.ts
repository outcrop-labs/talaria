import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { canDiscussDoc } from '@/server/kb-comments'
import { getRedis } from '@/server/db/redis'
import { db } from '@/server/db/pg'

const KEY_PREFIX = (docId: string) => `kb:presence:${docId}:`
const TTL = 45 // seconds — heartbeats land every ~25s

// Doc presence (the multiplayer layer's heartbeat). PUT { mode } → I'm here,
// viewing or editing. GET → who's here right now, with their mode — the doc
// header renders the avatar stack and the concurrent-edit warning from this.
export const Route = createFileRoute('/api/kb/docs/$id/live')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const gate = await requireUser(request)
        if (gate instanceof Response) return gate
        const user = gate
        if (!(await canDiscussDoc(params.id, user.id, user.email ?? user.name))) return json({ error: 'not found' }, { status: 404 })
        const body = await parseBody(request, z.object({ mode: z.enum(['view', 'edit']) }))
        if (body instanceof Response) return body
        const redis = getRedis()
        await redis.set(`${KEY_PREFIX(params.id)}${user.id}`, body.mode, 'EX', TTL)
        return json({ ok: true })
      },
      GET: async ({ request, params }) => {
        const gate = await requireUser(request)
        if (gate instanceof Response) return gate
        const user = gate
        if (!(await canDiscussDoc(params.id, user.id, user.email ?? user.name))) return json({ error: 'not found' }, { status: 404 })
        const redis = getRedis()
        const keys = await redis.keys(`${KEY_PREFIX(params.id)}*`)
        if (keys.length === 0) return json({ active: [] })
        const modes = await redis.mget(keys)
        const ids = keys.map((k) => k.slice(KEY_PREFIX(params.id).length))
        const sql = await db()
        const users = (await sql`select id, name, email from users where id = any(${ids}::uuid[])`) as unknown as Array<{
          id: string
          name: string | null
          email: string | null
        }>
        return json({
          active: ids
            .map((id, i) => {
              const u = users.find((x) => x.id === id)
              return u ? { userId: id, name: u.name ?? u.email ?? 'someone', mode: modes[i] === 'edit' ? 'edit' : 'view' } : null
            })
            .filter(Boolean),
        })
      },
    },
  },
})

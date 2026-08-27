import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { db } from '@/server/db/pg'

// Saved board views — named filter/layout presets shared with the board.
// GET → the board's views (any member). POST → create; PUT → rename/update
// config; DELETE → remove (owner/editor). Config is the board URL's search
// state verbatim (view/group/q/facets) — the client owns its meaning.
const Config = z.object({
  view: z.enum(['board', 'list', 'gantt']).optional(),
  group: z.string().max(20).optional(),
  q: z.string().max(200).optional(),
  status: z.string().max(300).optional(),
  assignee: z.string().max(2000).optional(),
  priority: z.string().max(100).optional(),
  label: z.string().max(1000).optional(),
  due: z.string().max(20).optional(),
})

const ROW = `id, board_id as "boardId", name, config, created_by as "createdBy", position,
  created_at as "createdAt", updated_at as "updatedAt"`

export const Route = defineApi('/api/boards/$id/views', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const sql = await db()
    const views = await sql.unsafe(`select ${ROW} from board_views where board_id = $1 order by position, created_at`, [params.id])
    return json({ views })
  },
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, z.object({ name: z.string().min(1).max(60), config: Config }))
    if (body instanceof Response) return body
    const sql = await db()
    const rows = await sql`
      insert into board_views (board_id, name, config, created_by,
        position)
      values (${params.id}, ${body.name}, ${sql.json(body.config)}, ${user.email ?? user.name ?? 'user'},
        coalesce((select max(position) + 1 from board_views where board_id = ${params.id}), 0))
      returning ${sql.unsafe(ROW)}
    `
    return json({ view: rows[0] })
  },
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(
      request,
      z.object({ viewId: Uuid, name: z.string().min(1).max(60).optional(), config: Config.optional() }),
    )
    if (body instanceof Response) return body
    const sql = await db()
    if (body.name !== undefined)
      await sql`update board_views set name = ${body.name}, updated_at = now() where id = ${body.viewId} and board_id = ${params.id}`
    if (body.config !== undefined)
      await sql`update board_views set config = ${sql.json(body.config)}, updated_at = now() where id = ${body.viewId} and board_id = ${params.id}`
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, z.object({ viewId: Uuid }))
    if (body instanceof Response) return body
    const sql = await db()
    await sql`delete from board_views where id = ${body.viewId} and board_id = ${params.id}`
    return json({ ok: true })
  },
})

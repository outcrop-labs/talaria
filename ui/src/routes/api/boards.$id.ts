import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { archiveBoard, boardRole, deleteBoard, renameBoard, setBoardJudgeMode, setBoardTeam } from '@/server/boards'
import { db } from '@/server/db/pg'
import { purgeActivityByField } from '@/server/retrieval/sources'
import { actingUser } from '@/server/users'

// PATCH /api/boards/:id { name?, archived?, judgeMode? } → rename/archive/set the
// QA judge mode (owner/editor). DELETE → owner only.
export const Route = createFileRoute('/api/boards/$id')({
  server: {
    handlers: {
      PATCH: async ({ request, params }) => {
        // Humans, or a personal assistant acting as its owner (identity proxy).
        const user = await actingUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        // An elevated assistant edits any board (never owner-level).
        const role = (await boardRole(user.id, params.id)) ?? (user.elevated ? 'editor' : null)
        if (role !== 'owner' && role !== 'editor') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({
            name: z.string().min(1).max(120).optional(),
            archived: z.boolean().optional(),
            judgeMode: z.enum(['inherit', 'off', 'advisory', 'enforcing']).optional(),
            /** Move between teams (null → personal). Owner only — it changes who can see the board. */
            teamId: z.string().uuid().nullable().optional(),
            /** Team by NAME (assistant-friendly): "personal" / "" / null → no team. */
            teamName: z.string().max(120).nullish(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        let teamId = parsed.data.teamId
        if (teamId === undefined && parsed.data.teamName !== undefined) {
          const name = (parsed.data.teamName ?? '').trim().toLowerCase()
          if (!name || name === 'personal') teamId = null
          else {
            const sql = await db()
            const rows = (await sql`select id from teams where lower(name) = ${name}`) as unknown as Array<{ id: string }>
            if (!rows[0]) return json({ error: `no team named "${parsed.data.teamName}"` }, { status: 400 })
            teamId = rows[0].id
          }
        }
        if (teamId !== undefined) {
          if (role !== 'owner') return json({ error: 'only the owner can move a board between teams' }, { status: 403 })
          try {
            await setBoardTeam(params.id, teamId)
          } catch (e) {
            return json({ error: (e as Error).message }, { status: 400 })
          }
        }
        if (parsed.data.name !== undefined) await renameBoard(params.id, parsed.data.name)
        if (parsed.data.archived !== undefined) await archiveBoard(params.id, parsed.data.archived)
        if (parsed.data.judgeMode !== undefined) await setBoardJudgeMode(params.id, parsed.data.judgeMode)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if ((await boardRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
        await deleteBoard(params.id)
        // Purge the board's tickets + comments from the activity brain.
        void purgeActivityByField('boardId', params.id).catch(() => {})
        return json({ ok: true })
      },
    },
  },
})

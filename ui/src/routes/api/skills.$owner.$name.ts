import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteSkill, readSkill, writeSkill } from '@/server/agent-skills'
import { ownsAgent } from '@/server/personal-agent'

const Body = z.object({ content: z.string().max(500_000) })

/** Admins write anywhere; a member may write their own assistant's skills. */
const canWrite = async (user: { id: string; role: string }, owner: string) =>
  user.role === 'admin' || ownsAgent(user.id, { slug: owner })

// One skill's SKILL.md. GET → content + file list. PUT → save (creates the
// skill if new). DELETE → remove the whole skill dir. Writes: admin, or the
// owner of a personal assistant for its own skills; edits are LIVE (Hermes
// reads skills per invocation, no restart).
export const Route = createFileRoute('/api/skills/$owner/$name')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        try {
          return json(await readSkill(params.owner, params.name))
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 404 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canWrite(user, params.owner))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          await writeSkill(params.owner, params.name, parsed.data.content, user.email ?? user.name ?? 'admin')
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canWrite(user, params.owner))) return json({ error: 'forbidden' }, { status: 403 })
        try {
          await deleteSkill(params.owner, params.name)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})

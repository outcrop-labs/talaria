import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { deleteSkill, readSkill, writeSkill } from '@/server/agent-skills'
import { ownsAgent } from '@/server/personal-agent'
import { deniedViews } from '@/server/users'

const Body = z.object({ content: z.string().max(500_000) })

/** Admins write anywhere; a member may write their own assistant's skills. */
const canWrite = async (user: { id: string; role: string }, owner: string) =>
  user.role === 'admin' || ownsAgent(user.id, { slug: owner })

/** Reads: writers, plus members granted the Agents manage view. */
const canRead = async (user: { id: string; role: 'admin' | 'member' }, owner: string) =>
  (await canWrite(user, owner)) || !(await deniedViews(user.id, user.role)).includes('/agents')

// One skill's SKILL.md. GET → content + file list. PUT → save (creates the
// skill if new). DELETE → remove the whole skill dir. Writes: admin, or the
// owner of a personal assistant for its own skills; edits are LIVE (Hermes
// reads skills per invocation, no restart).
export const Route = createFileRoute('/api/skills/$owner/$name')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await canRead(user, params.owner))) return json({ error: 'forbidden' }, { status: 403 })
        try {
          return json(await readSkill(params.owner, params.name))
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 404 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await canWrite(user, params.owner))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        try {
          await writeSkill(params.owner, params.name, body.content, user.email ?? user.name ?? 'admin')
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
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

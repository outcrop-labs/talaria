import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { deleteSkill, readSkill, writeSkill } from '@/server/agent-skills'
import { canEditSkills } from '@/server/skill-access'

const Body = z.object({ content: z.string().max(500_000) })

// One skill's SKILL.md. GET → content + file list (any member — the library
// is org work material). PUT → save (creates the skill if new). DELETE →
// remove the whole skill dir. Writes go through canEditSkills: admin /
// agents.manage everywhere; personal-assistant owners and explicit
// user_agent_access grantees for that agent's own skills. Edits are LIVE
// (Hermes reads skills per invocation, no restart).
export const Route = createFileRoute('/api/skills/$owner/$name')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        try {
          return json(await readSkill(params.owner, params.name))
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 404 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await canEditSkills(user, params.owner))) return json({ error: 'forbidden' }, { status: 403 })
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
        if (!(await canEditSkills(user, params.owner))) return json({ error: 'forbidden' }, { status: 403 })
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

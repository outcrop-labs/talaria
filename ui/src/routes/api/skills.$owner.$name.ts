import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { copySkill, deleteSkill, readSkill, renameSkill, writeSkill } from '@/server/agent-skills'
import { canEditSkill, canEditSkills } from '@/server/skill-access'

const Body = z.object({ content: z.string().max(500_000) })
const NAME = z.string().regex(/^[a-z0-9][a-z0-9._-]*$/).max(80)
const Op = z.union([
  z.object({ op: z.literal('rename'), toName: NAME }),
  z.object({ op: z.literal('copy'), toOwner: z.string().min(1).max(80), toName: NAME.optional() }),
  z.object({ op: z.literal('move'), toOwner: z.string().min(1).max(80), toName: NAME.optional() }),
])

// One skill's SKILL.md. GET → content + file list (any member — the library
// is org work material). PUT → save (creates the skill if new). DELETE →
// remove the whole skill dir. Writes go through canEditSkills: admin /
// agents.manage everywhere; personal-assistant owners and explicit
// user_agent_access grantees for that agent's own skills. Edits are LIVE
// (Hermes reads skills per invocation, no restart).
export const Route = defineApi('/api/skills/$owner/$name', {
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
    if (!(await canEditSkill(user, params.owner, params.name))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    try {
      await writeSkill(params.owner, params.name, body.content, user.email ?? user.name ?? 'admin')
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  // Structural ops: rename in place, copy/move to another owner (e.g.
  // promote an agent's skill to shared). Copy needs write on the
  // DESTINATION; rename/move also on the source.
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Op)
    if (body instanceof Response) return body
    const needSource = body.op !== 'copy'
    if (needSource && !(await canEditSkill(user, params.owner, params.name))) return json({ error: 'forbidden' }, { status: 403 })
    const dest = body.op === 'rename' ? params.owner : body.toOwner
    if (!(await canEditSkills(user, dest))) return json({ error: 'forbidden' }, { status: 403 })
    try {
      if (body.op === 'rename') await renameSkill(params.owner, params.name, body.toName)
      else await copySkill(params.owner, params.name, body.toOwner, { toName: body.toName, removeSource: body.op === 'move' })
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await canEditSkill(user, params.owner, params.name))) return json({ error: 'forbidden' }, { status: 403 })
    try {
      await deleteSkill(params.owner, params.name)
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})

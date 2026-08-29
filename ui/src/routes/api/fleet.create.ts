import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { randomUUID } from 'node:crypto'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requirePerm } from '@/server/api-guard'
import { db } from '@/server/db/pg'
import { agentHireRun } from '@/server/runs/defs/agent-hire'
import { drive, enqueue } from '@/server/runs/run'

const Body = z.object({
  slug: z.string().min(2).max(30),
  department: z.string().min(2).max(40),
  displayName: z.string().min(1).max(60),
  role: z.string().max(80).nullish(),
  /** Clone this agent's config; omit for the platform defaults. */
  templateId: Uuid.optional(),
  /** Override the starter-soul scaffold (e.g. an AI-designed soul). */
  soul: z.string().max(200_000).optional(),
  /** Starter skills written after creation (e.g. AI-designed playbooks). */
  skills: z
    .array(z.object({ name: z.string().regex(/^[a-z0-9][a-z0-9._-]*$/), content: z.string().max(100_000) }))
    .max(5)
    .optional(),
  start: z.boolean().optional(),
})

// POST → start HIRING a new agent. The work — create the def, write v1 and
// any starter skills, render the fleet, boot the container, wait out the
// healthcheck — is a durable `agent-hire` run, not this request: a boot runs
// to minutes on a cold pull, and a POST is a promise to stay on the line the
// modal cannot keep. The answer is the hire row; the roster shows the phases
// and the finished agent. Admin.
export const Route = defineApi('/api/fleet/create', {
  POST: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    // The one check that stays synchronous: a handle somebody can fix in the
    // open modal. Everything slower or rarer (template missing, bad config)
    // belongs to the run, where its sentence is visible on the roster.
    const sql = await db()
    const taken = await sql`select 1 from agent_defs where slug = ${body.slug}`
    if (taken.length) return json({ error: `an agent with the handle "${body.slug}" already exists` }, { status: 409 })

    const id = randomUUID()
    const row = await enqueue(
      agentHireRun,
      {
        slug: body.slug,
        department: body.department,
        displayName: body.displayName,
        role: body.role?.trim() || null,
        templateId: body.templateId ?? null,
        soul: body.soul?.trim() || null,
        skills: body.skills ?? [],
        start: body.start ?? true,
        actor: actorOf(user),
      },
      { id, ownerUserId: user.id, subjectType: 'agent-hire', subjectId: body.slug, phase: 'queued', start: false },
    )
    // The detached drive is the nicety; the reclaim sweep is the guarantee.
    void drive(id).catch((e) => console.error('[agent-hire] detached drive of', id, 'threw:', e))
    return json({ ok: true, hire: { id: row.id, state: row.state, phase: row.phase } })
  },
})

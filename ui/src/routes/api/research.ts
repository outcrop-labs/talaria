import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { listResearchRuns, RESEARCH_MODES, startResearch } from '@/server/research'
import { canUseAgentModel, personalAssistantOwners } from '@/server/users'
import { db } from '@/server/db/pg'

const Body = z.object({
  question: z.string().min(8).max(4000),
  mode: z.enum(['recon', 'brief', 'expedition']).default('brief'),
  /** Which agent's persona plans + synthesizes. Agent-key callers are pinned
   *  to themselves; humans pick (and need access to that agent). */
  agentModel: z.string().min(1).max(200).optional(),
})

// GET → recent research runs (org-visible: research is shared knowledge) +
// the mode catalog. POST { question, mode, agentModel? } → start a run.
// Humans and agents (fleet key) both start runs; an agent researches AS
// ITSELF, and its owner (for a personal assistant) gets the notification.
export const Route = createFileRoute('/api/research')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const isAgent = checkAgentKey(request)
        if (!isAgent && !(await getSessionUser(request))) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ runs: await listResearchRuns(), modes: RESEARCH_MODES })
      },
      POST: async ({ request }) => {
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })

        let agentModel: string
        let ownerUserId: string | null
        let requestedBy: string
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          agentModel = name // an agent researches in its own field
          ownerUserId = (await personalAssistantOwners()).get(name) ?? null
          requestedBy = name
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!parsed.data.agentModel) return json({ error: 'agentModel required' }, { status: 400 })
          agentModel = parsed.data.agentModel
          if (!(await canUseAgentModel(user.id, user.role, agentModel))) {
            return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
          }
          ownerUserId = user.id
          requestedBy = user.email ?? user.name ?? 'user'
        }

        // A queued/running duplicate of the same question is a double-click.
        const sql = await db()
        const dupe = await sql`
          select id from research_runs
          where question = ${parsed.data.question} and status in ('queued','running') limit 1
        `
        if (dupe[0]) return json({ run: null, duplicateOf: (dupe[0] as { id: string }).id }, { status: 409 })

        try {
          const run = await startResearch({
            question: parsed.data.question,
            mode: parsed.data.mode,
            agentModel,
            ownerUserId,
            requestedBy,
          })
          return json({ run })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})

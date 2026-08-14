import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { agentCaller } from '@/server/agent-auth'
import { listResearchRuns, RESEARCH_MODES, startResearch } from '@/server/research'
import { assistantOwnerFor, canUseAgentModel } from '@/server/users'
import { currentAgentTurn, rememberResearchOrigin } from '@/server/research-origin'
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
export const Route = defineApi('/api/research', {
  GET: async ({ request }) => {
    // Scope to the viewer: a user sees their own + shared + org runs; an
    // agent sees through its owner's eyes (general agents: org runs only).
    const viewer = await agentCaller(request)
    if (viewer instanceof Response) return viewer
    if (viewer) {
      // The CALLER: seeing a human's private runs is owner-proxying, so an
      // asserted identity resolves to no owner (org runs only).
      const owner = await assistantOwnerFor(viewer)
      return json({ runs: await listResearchRuns(owner), modes: RESEARCH_MODES })
    }
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    return json({ runs: await listResearchRuns(user.id), modes: RESEARCH_MODES })
  },
  POST: async ({ request }) => {
    const parsed = Body.safeParse(await request.json().catch(() => null))
    if (!parsed.success) return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })

    let agentModel: string
    let ownerUserId: string | null
    let requestedBy: string
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      const name = caller.model
      agentModel = name // an agent researches in its own field
      ownerUserId = await assistantOwnerFor(caller)
      requestedBy = name
    } else {
      const user = await getSessionUser(request)
      if (!user) return json({ error: 'unauthorized' }, { status: 401 })
      if (!(await hasPerm(user, 'research.run'))) return json({ error: 'no permission to run research' }, { status: 403 })
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
      // WHO IS OWED THE ANSWER. An agent that starts a run mid-conversation
      // cannot wait for it — the tool returns a runId and the turn ends — so the
      // run remembers the chat it came out of and reports back there when it
      // finishes. Only for agent callers: a human who started a run from the
      // Research page is already looking at the page that updates.
      if (caller) {
        const origin = await currentAgentTurn(agentModel)
        if (origin) await rememberResearchOrigin(run.id, origin)
      }
      return json({ run })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})

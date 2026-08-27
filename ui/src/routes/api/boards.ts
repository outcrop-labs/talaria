import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { agentCaller } from '@/server/agent-auth'
import { createBoard, listAllBoards, listBoards, listBoardsForAgent } from '@/server/boards'
import { teamRole } from '@/server/teams'
import { assistantOwnerFor, isElevatedAssistant } from '@/server/users'

// GET /api/boards → boards the user owns or that are shared with them.
// Agent-key + x-agent-name → boards whose policy allows that agent; a personal
// assistant additionally sees its owner's boards (with the owner's role) so it
// can govern them on the owner's behalf.
// POST /api/boards { name } → create a board (user becomes owner).
export const Route = defineApi('/api/boards', {
  GET: async ({ request }) => {
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      const agent = caller.model
      const policyBoards = await listBoardsForAgent(agent)
      // Owner-proxying and org-wide reach key off the CALLER: a legacy
      // shared-key caller only ever gets the boards its policy allows.
      const ownerId = await assistantOwnerFor(caller)
      if (!ownerId) return json({ boards: policyBoards })
      const ownerBoards = await listBoards(ownerId)
      const seen = new Set(ownerBoards.map((b) => b.id))
      // Elevated assistants see every live board org-wide (as editor).
      const rest = (await isElevatedAssistant(caller))
        ? (await listAllBoards()).map((b) => ({ ...b, role: 'editor' as const }))
        : policyBoards
      return json({ boards: [...ownerBoards, ...rest.filter((b) => !seen.has(b.id))] })
    }
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const archived = new URL(request.url).searchParams.get('archived') === '1'
    return json({ boards: await listBoards(user.id, archived) })
  },
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    // Authorization BEFORE body parsing — never do work for a caller who
    // can't take the action.
    if (!(await hasPerm(user, 'boards.create'))) return json({ error: 'no permission to create boards' }, { status: 403 })
    const body = await parseBody(
      request,
      z.object({ name: z.string().min(1).max(120), teamId: z.string().uuid().nullish() }),
    )
    if (body instanceof Response) return body
    // Team boards require membership in that team.
    if (body.teamId && !(await teamRole(user.id, body.teamId))) {
      return json({ error: 'not a member of that team' }, { status: 403 })
    }
    return json({ board: await createBoard(user.id, body.name, body.teamId ?? null) })
  },
})

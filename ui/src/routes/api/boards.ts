import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { createBoard, listAllBoards, listBoards, listBoardsForAgent } from '@/server/boards'
import { teamRole } from '@/server/teams'
import { isElevatedAssistant, personalAssistantOwners } from '@/server/users'

// GET /api/boards → boards the user owns or that are shared with them.
// Agent-key + x-agent-name → boards whose policy allows that agent; a personal
// assistant additionally sees its owner's boards (with the owner's role) so it
// can govern them on the owner's behalf.
// POST /api/boards { name } → create a board (user becomes owner).
export const Route = createFileRoute('/api/boards')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (checkAgentKey(request)) {
          const agent = agentName(request)
          if (!agent) return json({ error: 'x-agent-name required' }, { status: 400 })
          const policyBoards = await listBoardsForAgent(agent)
          const ownerId = (await personalAssistantOwners()).get(agent)
          if (!ownerId) return json({ boards: policyBoards })
          const ownerBoards = await listBoards(ownerId)
          const seen = new Set(ownerBoards.map((b) => b.id))
          // Elevated assistants see every live board org-wide (as editor).
          const rest = (await isElevatedAssistant(agent))
            ? (await listAllBoards()).map((b) => ({ ...b, role: 'editor' as const }))
            : policyBoards
          return json({ boards: [...ownerBoards, ...rest.filter((b) => !seen.has(b.id))] })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const archived = new URL(request.url).searchParams.get('archived') === '1'
        return json({ boards: await listBoards(user.id, archived) })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({ name: z.string().min(1).max(120), teamId: z.string().uuid().nullish() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // Team boards require membership in that team.
        if (parsed.data.teamId && !(await teamRole(user.id, parsed.data.teamId))) {
          return json({ error: 'not a member of that team' }, { status: 403 })
        }
        return json({ board: await createBoard(user.id, parsed.data.name, parsed.data.teamId ?? null) })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { createSpace, listSpaces } from '@/server/kb'
import { canRead, canReadAgent, grantedItemIds, grantedItemIdsForAgent } from '@/server/kb-perms'

const Body = z.object({ name: z.string().min(1).max(80), description: z.string().max(400).optional(), icon: z.string().max(8).optional() })

// KB spaces (any member). GET → all. POST → create.
export const Route = createFileRoute('/api/kb/spaces')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        // Agents (over MCP) see org/public spaces + ones granted to them.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          const granted = await grantedItemIdsForAgent('space', name)
          const spaces = (await listSpaces()).filter((s) => granted.has(s.id) || canReadAgent(s, name))
          return json({ spaces })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        // Hide folders the caller can't read, but keep ones shared with them.
        const granted = await grantedItemIds('space', user.id)
        const spaces = (await listSpaces()).filter((s) => granted.has(s.id) || canRead(s, user.id, user.email ?? user.name))
        return json({ spaces })
      },
      POST: async ({ request }) => {
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // Agents (over MCP) may create spaces too — a space is just a shelf,
        // and docs stay drafts until a human officializes them. No owner, so
        // sharing/deletion stay human calls.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          const dup = (await listSpaces()).find((s) => s.name.trim().toLowerCase() === parsed.data.name.trim().toLowerCase())
          if (dup) return json({ space: dup }) // find-or-create: agents retry; duplicates rot the KB
          return json({ space: await createSpace({ ...parsed.data, createdBy: name, ownerUserId: null }) })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ space: await createSpace({ ...parsed.data, createdBy: user.email ?? user.name ?? 'user', ownerUserId: user.id }) })
      },
    },
  },
})

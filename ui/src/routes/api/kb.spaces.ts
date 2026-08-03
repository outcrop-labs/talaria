import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { createSpace, listSpaces } from '@/server/kb'
import { canRead, canReadAgent, grantedItemIds, grantedItemIdsForAgent } from '@/server/kb-perms'
import { logAudit } from '@/server/audit'

const Body = z.object({ name: z.string().min(1).max(80), description: z.string().max(400).optional(), icon: z.string().max(8).optional() })

// KB spaces (any member). GET → all. POST → create.
export const Route = createFileRoute('/api/kb/spaces')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        // Agents (over MCP) see org/public spaces + ones granted to them.
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          const name = caller.model
          const granted = await grantedItemIdsForAgent('space', name)
          const spaces = (await listSpaces()).filter((s) => granted.has(s.id) || canReadAgent(s, name))
          return json({ spaces })
        }
        const gate = await requireUser(request)
        if (gate instanceof Response) return gate
        const user = gate
        // Hide folders the caller can't read, but keep ones shared with them.
        const granted = await grantedItemIds('space', user.id)
        const spaces = (await listSpaces()).filter((s) => granted.has(s.id) || canRead(s, user.id, user.email ?? user.name))
        return json({ spaces })
      },
      POST: async ({ request }) => {
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        // Agents (over MCP) may create spaces too — a space is just a shelf,
        // and docs stay drafts until a human officializes them. No owner, so
        // sharing/deletion stay human calls.
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          const name = caller.model
          const dup = (await listSpaces()).find((s) => s.name.trim().toLowerCase() === body.name.trim().toLowerCase())
          if (dup) return json({ space: dup }) // find-or-create: agents retry; duplicates rot the KB
          return json({ space: await createSpace({ ...body, createdBy: name, ownerUserId: null }) })
        }
        const gate = await requirePerm(request, 'kb.official')
        if (gate instanceof Response) return gate
        const user = gate
        const space = await createSpace({ ...body, createdBy: user.email ?? user.name ?? 'user', ownerUserId: user.id })
        void logAudit({ actor: actorOf(user), action: 'kb.space_create', targetType: 'kb-space', targetId: space.id, targetLabel: space.name })
        return json({ space })
      },
    },
  },
})

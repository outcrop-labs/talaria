import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { agentCaller } from '@/server/agent-auth'
import { createChannel, listChannels, listChannelsForAgent } from '@/server/channels'
import { maybeSweepTitles } from '@/server/titler'
import { ensureMcpService } from '@/server/mcp-service'
import { maybeRagSweep } from '@/server/retrieval/backfill'

// GET /api/channels → the user's channels/relays/DMs. POST { name, topic?,
// kind? } → create a channel (default) or a Relay (kind 'group').
export const Route = createFileRoute('/api/channels')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        // Agents see the channels they've been added to.
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          // The CALLER, not its model. `listChannelsForAgent` widens to EVERY
          // non-DM channel for an elevated assistant, and `subjectProven` reads
          // a bare string as proven — so `caller.model` here would throw the
          // legacy flag away and hand org-wide reach to an asserted identity.
          return json({ channels: await listChannelsForAgent(caller) })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        // Comms decay and the outreach sweep used to be kicked from here. They
        // are jobs on `server/scheduler.ts` now — timed by the process, not by
        // whether anyone happened to open comms. The three below are still
        // request-kicked and should follow (each lives in a file this change
        // does not own): `maybeSweepTitles` and `maybeRagSweep` have the same
        // "an idle instance never does it" bug, and `ensureMcpService` is a
        // supervisor, which is a scheduler job in everything but name.
        maybeSweepTitles() // retroactive + ongoing naming (hourly, detached)
        ensureMcpService() // keep the fleet's toolkit MCP endpoint alive (probe-guarded)
        maybeRagSweep() // incremental catch-up indexing (15-minute throttle)
        return json({ channels: await listChannels(user.id) })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({
            name: z.string().min(1).max(80),
            topic: z.string().max(300).nullish(),
            kind: z.enum(['channel', 'group']).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const needed = (parsed.data.kind ?? 'channel') === 'group' ? 'comms.relays' : 'comms.channels'
        if (!(await hasPerm(user, needed))) return json({ error: `no permission to create ${needed === 'comms.relays' ? 'relays' : 'channels'}` }, { status: 403 })
        return json({
          channel: await createChannel(user.id, parsed.data.name, parsed.data.topic ?? null, parsed.data.kind ?? 'channel'),
        })
      },
    },
  },
})

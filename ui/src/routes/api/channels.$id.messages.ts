import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { agentMayAccessChannel, channelRole, insertChannelMessage, listChannelMessages } from '@/server/channels'
import { notifyDmMessage, notifyUserMentions, triggerAgentReplies } from '@/server/channel-replies'
import { describeAgent } from '@/server/gateway'
import { resolveAttachments } from '@/server/uploads'
import { resolveRefs } from '@/server/refs'
import { indexActivity } from '@/server/retrieval/sources'
import { db } from '@/server/db/pg'

// GET ?since=<seq> → the channel's messages (members). POST { content } → post
// a message; @mentioned channel agents reply, streamed into the channel.
export const Route = createFileRoute('/api/channels/$id/messages')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const since = Number(new URL(request.url).searchParams.get('since') ?? -1)
        // Agents in the channel can read it (elevated assistants: any non-DM).
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !(await agentMayAccessChannel(params.id, name))) return json({ error: 'forbidden' }, { status: 403 })
          return json({ messages: await listChannelMessages(params.id, Number.isFinite(since) ? since : -1) })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json({ messages: await listChannelMessages(params.id, Number.isFinite(since) ? since : -1) })
      },
      POST: async ({ request, params }) => {
        const parsed = z
          .object({
            content: z.string().max(20_000).default(''),
            attachmentIds: z.array(z.string().uuid()).max(10).optional(),
            refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: z.string().uuid() })).max(3).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success || (!parsed.data.content && !parsed.data.attachmentIds?.length && !parsed.data.refs?.length)) {
          return json({ error: 'bad request' }, { status: 400 })
        }

        // An agent in the channel can post. It doesn't trigger other agents (no
        // reply storms) and can't attach uploads.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name || !(await agentMayAccessChannel(params.id, name))) return json({ error: 'forbidden' }, { status: 403 })
          if (!parsed.data.content.trim()) return json({ error: 'bad request' }, { status: 400 })
          const msg = await insertChannelMessage(params.id, 'agent', name, parsed.data.content, 'complete')
          const sql0 = await db()
          const nm = ((await sql0`select name from channels where id = ${params.id}`)[0] as { name: string } | undefined)?.name ?? 'channel'
          void indexActivity({ sourceType: 'channel', sourceId: msg.id, title: `#${nm} · ${name}`, text: parsed.data.content, payload: { channelId: params.id }, href: '/channels' }).catch(() => {})
          // An agent @mentioning a human notifies exactly like a human would.
          void notifyUserMentions(params.id, nm, '', describeAgent(name).label, parsed.data.content).catch(() => {})
          return json({ message: msg })
        }

        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const author = user.email ?? user.name ?? 'user'
        const uploads = await resolveAttachments(parsed.data.attachmentIds ?? [])
        const refChips = await resolveRefs(user, parsed.data.refs ?? [])
        const message = await insertChannelMessage(params.id, 'user', author, parsed.data.content, 'complete', [...uploads, ...refChips])

        // Agent replies + mention notifications run detached; the POST returns at once.
        const sql = await db()
        const rows = await sql`select name from channels where id = ${params.id}`
        const channelName = (rows[0] as { name: string } | undefined)?.name ?? 'channel'

        // Index into the ambient activity brain (retrieval on demand later).
        if (parsed.data.content.trim()) {
          void indexActivity({
            sourceType: 'channel',
            sourceId: message.id,
            title: `#${channelName} · ${author}`,
            text: parsed.data.content,
            payload: { channelId: params.id },
            href: '/channels',
          }).catch(() => {})
        }
        void triggerAgentReplies(params.id, channelName, parsed.data.content).catch(() => {})
        // A DM message notifies the peer outright (deduped while unread);
        // channel/relay messages notify only on @mention.
        const kind = ((await sql`select kind from channels where id = ${params.id}`)[0] as { kind?: string } | undefined)?.kind
        if (kind === 'dm') {
          void notifyDmMessage(params.id, user.id, user.name ?? author, parsed.data.content).catch(() => {})
        } else {
          void notifyUserMentions(params.id, channelName, user.id, user.name ?? author, parsed.data.content).catch(() => {})
        }
        return json({ message })
      },
    },
  },
})

import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { channelRole, insertChannelMessage, listChannelMessages } from '@/server/channels'
import { notifyUserMentions, triggerAgentReplies } from '@/server/channel-replies'
import { resolveAttachments } from '@/server/uploads'
import { indexActivity } from '@/server/retrieval/sources'
import { db } from '@/server/db/pg'

// GET ?since=<seq> → the channel's messages (members). POST { content } → post
// a message; @mentioned channel agents reply, streamed into the channel.
export const Route = createFileRoute('/api/channels/$id/messages')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const since = Number(new URL(request.url).searchParams.get('since') ?? -1)
        return json({ messages: await listChannelMessages(params.id, Number.isFinite(since) ? since : -1) })
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ content: z.string().max(20_000).default(''), attachmentIds: z.array(z.string().uuid()).max(10).optional() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success || (!parsed.data.content && !parsed.data.attachmentIds?.length)) {
          return json({ error: 'bad request' }, { status: 400 })
        }
        const author = user.email ?? user.name ?? 'user'
        const attachments = await resolveAttachments(parsed.data.attachmentIds ?? [])
        const message = await insertChannelMessage(params.id, 'user', author, parsed.data.content, 'complete', attachments)

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
        void notifyUserMentions(params.id, channelName, user.id, user.name ?? author, parsed.data.content).catch(() => {})
        return json({ message })
      },
    },
  },
})

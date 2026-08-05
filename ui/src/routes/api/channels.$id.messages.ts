import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller } from '@/server/agent-auth'
import { agentMayAccessChannel, channelRole, getChannelMessage, insertChannelMessage, listChannelMessages, listThreadMessages } from '@/server/channels'
import { notifyDmMessage, notifyUserMentions, triggerAgentReplies } from '@/server/channel-replies'
import { describeAgent } from '@/server/gateway'
import { resolveAttachments } from '@/server/uploads'
import { resolveRefs } from '@/server/refs'
import { indexActivity } from '@/server/retrieval/sources'
import { db } from '@/server/db/pg'

// GET ?since=<seq> → the channel's messages (members). POST { content } → post
// a message; @mentioned channel agents reply, streamed into the channel.
export const Route = defineApi('/api/channels/$id/messages', {
  GET: async ({ request, params }) => {
    const url = new URL(request.url)
    const since = Number(url.searchParams.get('since') ?? -1)
    const thread = url.searchParams.get('thread')
    const page = () =>
      thread
        ? listThreadMessages(params.id, thread)
        : listChannelMessages(params.id, Number.isFinite(since) ? since : -1)
    // Agents in the channel can read it (elevated assistants: any non-DM).
    const reader = await agentCaller(request)
    if (reader instanceof Response) return reader
    if (reader) {
      // The CALLER, not its model — the elevated "any non-DM channel"
      // bypass is only for a proven identity.
      if (!(await agentMayAccessChannel(params.id, reader))) return json({ error: 'forbidden' }, { status: 403 })
      return json({ messages: await page() })
    }
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    return json({ messages: await page() })
  },
  POST: async ({ request, params }) => {
    const parsed = z
      .object({
        content: z.string().max(20_000).default(''),
        attachmentIds: z.array(z.string().uuid()).max(10).optional(),
        refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: z.string().uuid() })).max(3).optional(),
        threadRootId: z.string().uuid().nullish(),
      })
      .safeParse(await request.json().catch(() => null))
    if (!parsed.success || (!parsed.data.content && !parsed.data.attachmentIds?.length && !parsed.data.refs?.length)) {
      return json({ error: 'bad request' }, { status: 400 })
    }

    // An agent in the channel can post. It doesn't trigger other agents (no
    // reply storms) and can't attach uploads.
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      const name = caller.model
      // The CALLER, not `name`: elevation buys org-wide posting rights.
      if (!(await agentMayAccessChannel(params.id, caller))) return json({ error: 'forbidden' }, { status: 403 })
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
    // A thread reply hangs off a ROOT in this channel; replying to a reply
    // re-roots onto its thread (Slack semantics — threads never nest).
    let threadRootId: string | null = null
    if (parsed.data.threadRootId) {
      const root = await getChannelMessage(params.id, parsed.data.threadRootId)
      if (!root) return json({ error: 'no such thread' }, { status: 400 })
      threadRootId = root.threadRootId ?? root.id
    }
    const author = user.email ?? user.name ?? 'user'
    const uploads = await resolveAttachments(parsed.data.attachmentIds ?? [])
    const refChips = await resolveRefs(user, parsed.data.refs ?? [])
    const message = await insertChannelMessage(params.id, 'user', author, parsed.data.content, 'complete', [...uploads, ...refChips], threadRootId)

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
    void triggerAgentReplies(params.id, channelName, parsed.data.content, threadRootId).catch(() => {})
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
})

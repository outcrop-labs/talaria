import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
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
      // WITHOUT `guard`. An agent reads a channel through this route (the MCP
      // `read_channel` tool proxies it), and a finding is the guard's verdict on
      // flagged content plus a verbatim excerpt OF that content — the one thing
      // guardrails.ts's cardinal invariant says must never re-enter a model's
      // context. `channels.ts` declines to pin findings for exactly this reason
      // and says so; the streamed-reply path pins them anyway, so the projection
      // is where this is closed for good. Humans still get the caveat.
      return json({ messages: (await page()).map(({ guard: _guard, ...m }) => m) })
    }
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    return json({ messages: await page() })
  },
  POST: async ({ request, params }) => {
    const body = await parseBody(
      request,
      z.object({
        content: z.string().max(20_000).default(''),
        attachmentIds: z.array(z.string().uuid()).max(10).optional(),
        refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: z.string().uuid() })).max(3).optional(),
        threadRootId: z.string().uuid().nullish(),
      }),
    )
    if (body instanceof Response) return body
    // A post needs something in it: text, an attachment, or a ref chip.
    if (!body.content && !body.attachmentIds?.length && !body.refs?.length) {
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
      if (!body.content.trim()) return json({ error: 'bad request' }, { status: 400 })
      const msg = await insertChannelMessage(params.id, 'agent', name, body.content, 'complete')
      const sql0 = await db()
      const nm = ((await sql0`select name from channels where id = ${params.id}`)[0] as { name: string } | undefined)?.name ?? 'channel'
      // `msg.content`, NOT `body.content`. `insertChannelMessage` sends an
      // agent's post through the agent-writes door, which in strict mode returns
      // the REDACTED body — so the row in `channel_messages` is clean and these
      // two copies of the same text were the raw one.
      //
      // The index is the half that matters. Retrieval is read back INTO model
      // contexts, so an unredacted copy there is not merely a second place the
      // credential is stored: it is the credential re-entering a model's context
      // by the one route guardrails.ts exists to close, arriving as ambient
      // "activity" long after the turn that leaked it. The notification is the
      // same text landing in a human's inbox, unredacted, beside a message that
      // is not.
      void indexActivity({ sourceType: 'channel', sourceId: msg.id, title: `#${nm} · ${name}`, text: msg.content, payload: { channelId: params.id }, href: '/channels' }).catch(() => {})
      // An agent @mentioning a human notifies exactly like a human would.
      void notifyUserMentions(params.id, nm, '', describeAgent(name).label, msg.content).catch(() => {})
      return json({ message: msg })
    }

    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    // A thread reply hangs off a ROOT in this channel; replying to a reply
    // re-roots onto its thread (Slack semantics — threads never nest).
    let threadRootId: string | null = null
    if (body.threadRootId) {
      const root = await getChannelMessage(params.id, body.threadRootId)
      if (!root) return json({ error: 'no such thread' }, { status: 400 })
      threadRootId = root.threadRootId ?? root.id
    }
    const author = user.email ?? user.name ?? 'user'
    const uploads = await resolveAttachments(body.attachmentIds ?? [])
    const refChips = await resolveRefs(user, body.refs ?? [])
    const message = await insertChannelMessage(params.id, 'user', author, body.content, 'complete', [...uploads, ...refChips], threadRootId)

    // Agent replies + mention notifications run detached; the POST returns at once.
    const sql = await db()
    const rows = await sql`select name from channels where id = ${params.id}`
    const channelName = (rows[0] as { name: string } | undefined)?.name ?? 'channel'

    // Index into the ambient activity brain (retrieval on demand later).
    if (body.content.trim()) {
      void indexActivity({
        sourceType: 'channel',
        sourceId: message.id,
        title: `#${channelName} · ${author}`,
        text: body.content,
        payload: { channelId: params.id },
        href: '/channels',
      }).catch(() => {})
    }
    void triggerAgentReplies(params.id, channelName, body.content, threadRootId).catch(() => {})
    // A DM message notifies the peer outright (deduped while unread);
    // channel/relay messages notify only on @mention.
    const kind = ((await sql`select kind from channels where id = ${params.id}`)[0] as { kind?: string } | undefined)?.kind
    if (kind === 'dm') {
      void notifyDmMessage(params.id, user.id, user.name ?? author, body.content).catch(() => {})
    } else {
      void notifyUserMentions(params.id, channelName, user.id, user.name ?? author, body.content).catch(() => {})
    }
    return json({ message })
  },
})

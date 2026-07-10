// Distill-then-archive: conversations with agents don't accumulate forever.
// Idle agent DMs get their durable substance summarized into the activity
// brain (owner-scoped), then archive out of the sidebar — context survives,
// scrollback doesn't. Relays conclude explicitly: summary posted + indexed,
// then the relay archives. Everything here is fire-and-forget friendly.
import { db } from './db/pg'
import { agentCategoryFolder, createArtifact, saveArtifact } from './artifacts'
import { archiveChannel, insertChannelMessage, listChannelAgents, listChannelMessages } from './channels'
import { describeAgent } from './gateway'
import { completeViaGateway } from './llm-gateway'
import { museModelFor } from './muse'
import { indexActivity } from './retrieval/sources'

const TTL_DAYS = () => Math.max(1, Number(process.env.TALARIA_CHAT_TTL_DAYS ?? 14))
const SWEEP_BATCH = 8

const DISTILL_PROMPT =
  'Distill this conversation into its durable substance: decisions made, facts established, preferences expressed, and outcomes — terse markdown bullets, grouped when helpful. ' +
  'Skip pleasantries and process chatter. Never invent anything. Reply with ONLY the distillation.'

const clip = (s: string, max = 60_000) => (s.length > max ? `${s.slice(0, max)}\n…(truncated)` : s)

/** Distill one idle agent DM into the activity brain, then archive it. */
async function distillConversation(conv: {
  id: string
  userId: string
  agentModel: string
  title: string | null
}): Promise<void> {
  const sql = await db()
  const model = await museModelFor(conv.userId)
  if (!model) return // no routable model — leave it for a sweep that has one
  const msgs = (await sql`
    select role, content from messages
    where conversation_id = ${conv.id} and content <> '' order by seq asc
  `) as unknown as Array<{ role: string; content: string }>
  const label = describeAgent(conv.agentModel).label
  const transcript = clip(msgs.map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`).join('\n\n'))

  if (transcript.trim()) {
    const { text } = await completeViaGateway(
      model,
      [
        { role: 'system', content: DISTILL_PROMPT },
        { role: 'user', content: `Conversation with ${label}:\n\n${transcript}` },
      ],
      { temperature: 0.2, caller: `distill:${conv.userId}` },
    )
    if (!text.trim()) return // don't archive on a failed distillation
    const title = `Distilled: ${conv.title || `chat with ${label}`}`
    await indexActivity({
      sourceType: 'chat-distill',
      sourceId: conv.id,
      title,
      text,
      payload: { ownerUserId: conv.userId },
      href: '/comms',
    })
    // The distill is also a browsable artifact — PRIVATE to the chat's owner
    // (a DM's substance is theirs), filed under the agent's "Chat summaries".
    try {
      const artifact = await createArtifact({
        kind: 'doc',
        title,
        createdBy: label,
        ownerUserId: conv.userId,
        folderId: await agentCategoryFolder(label, 'Chat summaries', label),
      })
      await saveArtifact(artifact.id, { body: text }, label)
    } catch {
      /* filing is best-effort — the distillation is already indexed */
    }
  }
  await sql`update conversations set archived = true where id = ${conv.id}`
}

/** One pass: distill + archive up to SWEEP_BATCH idle agent DMs. Plans are
 *  exempt — they're durable documents, not chat scrollback. */
export async function sweepIdleChats(): Promise<number> {
  const sql = await db()
  const idle = (await sql`
    select id, user_id as "userId", agent_model as "agentModel", title
    from conversations
    where kind = 'chat' and archived = false
      and updated_at < now() - make_interval(days => ${TTL_DAYS()})
    order by updated_at asc
    limit ${SWEEP_BATCH}
  `) as unknown as Array<{ id: string; userId: string; agentModel: string; title: string | null }>
  let done = 0
  for (const conv of idle) {
    try {
      await distillConversation(conv)
      done++
    } catch {
      /* upstream hiccup — the next sweep retries this conversation */
    }
  }
  return done
}

// Opportunistic scheduling, mirroring maybeRefreshAutoPrices: any comms read
// may kick a sweep, at most once an hour, never blocking the request.
let lastSweep = 0
export function maybeSweepIdleChats(): void {
  const now = Date.now()
  if (now - lastSweep < 60 * 60_000) return
  lastSweep = now
  void sweepIdleChats().catch(() => {})
}

/** Conclude a Relay: post + index a summary of what was decided, then archive.
 *  Returns the summary so the UI can show it after the relay leaves the list. */
export async function concludeRelay(channelId: string, byUserId: string, channelName: string): Promise<string> {
  const model = await museModelFor(byUserId)
  if (!model) throw new Error('no model configured to summarize with — add an endpoint on /models')
  const history = await listChannelMessages(channelId, -1, 500)
  const transcript = clip(
    history
      .filter((m) => m.status === 'complete' && m.content)
      .map((m) => `${m.authorType === 'agent' ? describeAgent(m.author).label : m.author}: ${m.content}`)
      .join('\n\n'),
  )
  if (!transcript.trim()) throw new Error('nothing to conclude — the relay has no messages')

  const { text } = await completeViaGateway(
    model,
    [
      {
        role: 'system',
        content:
          'Write the closing summary for a work discussion: what was decided, what was produced, and any follow-ups — crisp markdown, a few bullets per section, no preamble.',
      },
      { role: 'user', content: `Relay "${channelName}":\n\n${transcript}` },
    ],
    { temperature: 0.2, caller: `relay-conclude:${byUserId}` },
  )
  if (!text.trim()) throw new Error('the summary came back empty — try again')

  // The summary is the relay's last word: posted into history (visible if the
  // relay is ever revisited) and indexed for retrieval (channel-membership ACL).
  const agents = await listChannelAgents(channelId)
  await insertChannelMessage(channelId, 'agent', agents[0] ?? 'talaria', `**Relay concluded** — summary:\n\n${text}`)
  await indexActivity({
    sourceType: 'relay-summary',
    sourceId: channelId,
    title: `Relay concluded: ${channelName}`,
    text,
    payload: { channelId },
    href: '/comms',
  })
  await archiveChannel(channelId)
  return text
}

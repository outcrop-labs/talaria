// The Titler platform agent — names things as they take shape. Chats and
// plans get retitled once their first exchange lands (only while the title is
// still the mechanical first-message truncation — a name a user typed or
// another flow chose is never clobbered); research runs get a title from
// their question the moment they start. Everything here is fire-and-forget:
// naming must never block or fail the work it names.
import { db } from './db/pg'
import { completeViaGateway, gatewayModels, resolveRoute } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { platformAgentModel } from './platform-agents'

const PROMPT: Record<'chat' | 'plan' | 'research', string> = {
  chat:
    'Name this conversation. 3–7 words, specific to what it is actually about — the subject, not the activity. ' +
    'No quotes, no trailing punctuation, never generic fillers like "Chat about" or "Discussion of". Reply with ONLY the title.',
  plan:
    'Name this plan. 3–7 words, outcome-focused — what the plan will deliver, not the conversation around it. ' +
    'No quotes, no trailing punctuation. Reply with ONLY the title.',
  research:
    'Name this research run from its question. 3–7 words capturing the subject under investigation. ' +
    'No quotes, no trailing punctuation, do not restate it as a question. Reply with ONLY the title.',
}

const clip = (s: string, max = 4000) => (s.length > max ? s.slice(0, max) : s)

async function titlerModel(): Promise<string | null> {
  const pinned = await platformAgentModel('titler')
  if (pinned) return pinned
  const utility = await resolveRoleModel('utility')
  if (utility) return utility
  for (const m of [process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']) {
    if (m && (await resolveRoute(m))) return m
  }
  return (await gatewayModels()).find((m) => !m.qualified)?.id ?? null
}

/** One short completion → a clean title, or null when nothing routes / the
 *  model rambles. Callers keep their existing title on null. */
export async function generateTitle(kind: 'chat' | 'plan' | 'research', text: string): Promise<string | null> {
  if (!text.trim()) return null
  const model = await titlerModel()
  if (!model) return null
  let out: string
  try {
    out = (
      await completeViaGateway(
        model,
        [
          { role: 'system', content: PROMPT[kind] },
          { role: 'user', content: clip(text) },
        ],
        { temperature: 0.3, caller: 'platform:titler' },
      )
    ).text
  } catch {
    return null // upstream hiccup (rate limit, dead route) — keep the current title
  }
  const t = (out.split('\n').find((l) => l.trim()) ?? '')
    .replace(/^["'#*\s]+|["'*\s]+$/g, '')
    .replace(/[.。]$/, '')
    .trim()
  if (!t) return null
  return t.length > 90 ? `${t.slice(0, 90).trimEnd()}…` : t
}

/** The mechanical default chat.ts stamps at creation — a title still equal to
 *  it means nobody has named the conversation on purpose. */
const mechanicalFrom = (s: string) => s.replace(/\s+/g, ' ').trim().slice(0, 80)

/** Retitle a chat/plan once its first exchange completes. Cheap early-outs:
 *  only within the first few messages, and only while the title is still the
 *  truncated first message (or the bare 'chat' fallback). */
export async function maybeRetitleConversation(conversationId: string): Promise<void> {
  const sql = await db()
  const [conv] = (await sql`
    select c.title, c.kind,
           (select count(*) from messages m where m.conversation_id = c.id) as count
    from conversations c where c.id = ${conversationId}
  `) as unknown as Array<{ title: string | null; kind: 'chat' | 'plan'; count: number }>
  if (!conv || Number(conv.count) > 4) return
  const msgs = (await sql`
    select role, content from messages
    where conversation_id = ${conversationId} and content <> '' order by seq asc limit 3
  `) as unknown as Array<{ role: string; content: string }>
  const firstUser = msgs.find((m) => m.role === 'user')
  const stillMechanical =
    !conv.title || conv.title === 'chat' || (firstUser && conv.title === mechanicalFrom(firstUser.content))
  if (!stillMechanical) return
  const transcript = msgs.map((m) => `${m.role}: ${m.content.slice(0, 1500)}`).join('\n\n')
  const title = await generateTitle(conv.kind === 'plan' ? 'plan' : 'chat', transcript)
  if (title) await sql`update conversations set title = ${title} where id = ${conversationId}`
}

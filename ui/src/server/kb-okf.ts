// Per-document OKF — each promoted doc carries a hidden agent-facing summary
// in the Open Knowledge Format (YAML frontmatter + markdown concept body, per
// the OKF spec): type/title/description/resource/tags, a `generated` trust
// stamp, and lifecycle status. The LIBRARIAN platform agent writes it when a
// doc is promoted, refreshes it when promoted content changes, and it clears
// on demotion. Agents read it through the doc API; humans peek via the OKF
// chip in the doc header.
import { db } from './db/pg'
import { completeViaGateway, gatewayModels, resolveRoute } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { platformAgentModel } from './platform-agents'
import { getDoc } from './kb'

const clip = (s: string, max = 12_000) => (s.length > max ? `${s.slice(0, max)}\n…(truncated)` : s)

async function librarianModel(): Promise<string | null> {
  const pinned = await platformAgentModel('librarian')
  if (pinned) return pinned
  const utility = await resolveRoleModel('utility')
  if (utility) return utility
  for (const m of [process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']) {
    if (m && (await resolveRoute(m))) return m
  }
  return (await gatewayModels()).find((m) => !m.qualified)?.id ?? null
}

const PROMPT =
  'You are the librarian writing the agent-facing summary BODY for a knowledge document (OKF concept body). ' +
  'Write: a 2-4 sentence summary of the document’s substance, then a "## Key facts" bullet list of the concrete facts, names, numbers, and decisions an agent would need without reading the full document. ' +
  'Also propose up to 5 lowercase topic tags on a final line formatted exactly as: TAGS: tag1, tag2. ' +
  'Factual, terse, no invention. Reply with ONLY the body and the TAGS line.'

/** (Re)generate one doc's OKF. No-op for unpromoted docs (demote clears). */
export async function generateDocOkf(docId: string): Promise<void> {
  const sql = await db()
  const doc = await getDoc(docId)
  if (!doc) return
  if (!doc.official) {
    await sql`update kb_docs set okf = null where id = ${docId}`
    return
  }
  const model = await librarianModel()
  if (!model || !doc.body.trim()) return
  const { text } = await completeViaGateway(
    model,
    [
      { role: 'system', content: PROMPT },
      { role: 'user', content: `Document "${doc.title}":\n\n${clip(doc.body)}` },
    ],
    { temperature: 0.2, caller: 'platform:librarian' },
  )
  if (!text.trim()) return // model hiccup — keep the previous OKF

  const tagsMatch = /^TAGS:\s*(.+)$/m.exec(text)
  const tags = (tagsMatch?.[1] ?? '')
    .split(',')
    .map((t) => t.trim().toLowerCase().replace(/[^a-z0-9-]/g, ''))
    .filter(Boolean)
    .slice(0, 5)
  const body = text.replace(/^TAGS:.*$/m, '').trim()
  const description = body.split('\n').find((l) => l.trim())?.slice(0, 160) ?? doc.title

  // OKF concept per the spec: frontmatter (type/title/description/resource/
  // tags + generated trust stamp + lifecycle) over a markdown body.
  const okf = [
    '---',
    'type: Knowledge Document',
    `title: ${JSON.stringify(doc.title)}`,
    `description: ${JSON.stringify(description)}`,
    `resource: /knowledge?d=${doc.id}`,
    tags.length ? `tags: [${tags.join(', ')}]` : null,
    `generated: { by: talaria/librarian:${model}, at: ${new Date().toISOString()} }`,
    'status: stable',
    'sources:',
    `  - resource: /knowledge?d=${doc.id}`,
    `    last_modified: ${doc.updatedAt}`,
    '---',
    '',
    body,
  ]
    .filter((l): l is string => l !== null)
    .join('\n')

  await sql`update kb_docs set okf = ${okf} where id = ${docId}`
}

// ── Debounced autonomy: promotions/saves queue their doc; bursts collapse. ──
const timers = new Map<string, ReturnType<typeof setTimeout>>()
export function queueDocOkf(docId: string): void {
  const prior = timers.get(docId)
  if (prior) clearTimeout(prior)
  timers.set(
    docId,
    setTimeout(() => {
      timers.delete(docId)
      void generateDocOkf(docId).catch(() => {})
    }, 15_000),
  )
}

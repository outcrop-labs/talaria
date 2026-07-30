// The Summarizer platform agent's one job: a single plain line per skill
// saying what it teaches, shown under the title everywhere skills are listed.
// Summaries persist keyed to a hash of the SKILL.md, so a skill is summarized
// exactly once per version — list calls serve the stored line and quietly
// queue a regeneration when the content changed. Never blocks a listing.
import { createHash } from 'node:crypto'
import { db } from './db/pg'
import { completeViaGateway, gatewayModels, resolveRoute } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { platformAgentModel } from './platform-agents'

const PROMPT =
  'Summarize this agent skill in ONE sentence (max 140 chars): what kind of work it covers and the gist of how. ' +
  'Plain words, no markdown, no "This skill…" lead-in — start with the substance. Reply with ONLY the sentence.'

export const skillHash = (md: string): string => createHash('sha1').update(md).digest('hex')

export interface StoredSummary {
  hash: string
  summary: string
}

/** All stored summaries in one query — listAllSkills consults this map. */
export async function storedSummaries(): Promise<Map<string, StoredSummary>> {
  const sql = await db()
  const rows = (await sql`select owner, name, hash, summary from skill_summaries`) as unknown as Array<{
    owner: string
    name: string
    hash: string
    summary: string
  }>
  return new Map(rows.map((r) => [`${r.owner}/${r.name}`, { hash: r.hash, summary: r.summary }]))
}

async function summarizerModel(): Promise<string | null> {
  const pinned = await platformAgentModel('summarizer')
  if (pinned) return pinned
  const utility = await resolveRoleModel('utility')
  if (utility) return utility
  for (const m of [process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']) {
    if (m && (await resolveRoute(m))) return m
  }
  return (await gatewayModels()).find((m) => !m.qualified)?.id ?? null
}

const inFlight = new Set<string>()

/** Fire-and-forget: regenerate one skill's summary for this content hash. */
export function queueSummary(owner: string, name: string, md: string): void {
  const key = `${owner}/${name}`
  if (inFlight.has(key) || !md.trim()) return
  inFlight.add(key)
  void (async () => {
    const hash = skillHash(md)
    const model = await summarizerModel()
    if (!model) return
    const out = await completeViaGateway(
      model,
      [
        { role: 'system', content: PROMPT },
        { role: 'user', content: md.slice(0, 6000) },
      ],
      { temperature: 0.3, caller: 'platform:summarizer' },
    ).catch((e: Error) => {
      console.warn('[summarizer] gateway', key, model, e.message)
      return null
    })
    const line = out?.text
      .split('\n')
      .map((l) => l.trim())
      .find(Boolean)
      ?.replace(/^["'#*\s]+|["'\s]+$/g, '')
      .slice(0, 180)
    if (!line) return
    const sql = await db()
    await sql`
      insert into skill_summaries (owner, name, hash, summary)
      values (${owner}, ${name}, ${hash}, ${line})
      on conflict (owner, name) do update set hash = ${hash}, summary = ${line}, updated_at = now()
    `
  })()
    .catch((e: Error) => console.warn('[summarizer]', key, e.message))
    .finally(() => inFlight.delete(key))
}

/** Housekeeping when skills move or die — the summary follows the file. */
export async function dropSummary(owner: string, name: string): Promise<void> {
  const sql = await db()
  await sql`delete from skill_summaries where owner = ${owner} and name = ${name}`
}

export async function moveSummary(owner: string, name: string, toOwner: string, toName: string): Promise<void> {
  const sql = await db()
  await sql`
    update skill_summaries set owner = ${toOwner}, name = ${toName}
    where owner = ${owner} and name = ${name}
  `.catch(async () => {
    await sql`delete from skill_summaries where owner = ${owner} and name = ${name}`
  })
}

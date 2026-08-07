// The Summarizer platform agent's one job: a single plain line per skill
// saying what it teaches, shown under the title everywhere skills are listed.
// Summaries persist keyed to a hash of the SKILL.md, so a skill is summarized
// exactly once per version — list calls serve the stored line and quietly
// queue a regeneration when the content changed. Never blocks a listing.
//
// The prompt, the model chain and the one-line extraction now live in
// harness/defs/summarizer.ts; this file is the STORAGE half — the content hash,
// the in-flight dedupe and the upsert — which is genuinely its own job and is
// not the harness's business.
import { createHash } from 'node:crypto'
import { db } from './db/pg'
import { summarizerHarness } from './harness/defs/summarizer'
import { runHarness } from './harness/run'

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

const inFlight = new Set<string>()

/** Fire-and-forget: regenerate one skill's summary for this content hash.
 *
 *  The dedupe set is NOT the harness's concern and stays here: it stops the same
 *  skill being summarized twice while one call is still out, which happens on
 *  every listing because a list call queues a regeneration for every changed
 *  skill it walks past. */
export function queueSummary(owner: string, name: string, md: string): void {
  const key = `${owner}/${name}`
  if (inFlight.has(key) || !md.trim()) return
  inFlight.add(key)
  void (async () => {
    const hash = skillHash(md)
    // `onFailure: 'null'` — no model on the gateway, an unusable reply, a dead
    // endpoint: all of them land here as a null value and nothing is written, so
    // the previously stored summary survives. The runner has already recorded
    // the attempt (model, chain step, latency, whether the contract held) on a
    // harness_runs row, which is where a failure belongs now; the console.warn
    // this replaced said less than that row does and said it to nobody.
    const { value: line } = await runHarness(summarizerHarness, { md }, { caller: 'platform:summarizer' })
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

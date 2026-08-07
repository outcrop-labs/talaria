// Per-document OKF — each promoted doc carries a hidden agent-facing summary
// in the Open Knowledge Format (YAML frontmatter + markdown concept body, per
// the OKF spec): type/title/description/resource/tags, a `generated` trust
// stamp, and lifecycle status. The LIBRARIAN platform agent writes it when a
// doc is promoted, refreshes it when promoted content changes, and it clears
// on demotion. Agents read it through the doc API; humans peek via the OKF
// chip in the doc header.
import { db } from './db/pg'
import { librarianHarness } from './harness/defs/librarian'
import { runHarness } from './harness/run'
import { getDoc } from './kb'

/** (Re)generate one doc's OKF. No-op for unpromoted docs (demote clears). */
export async function generateDocOkf(docId: string): Promise<void> {
  const sql = await db()
  const doc = await getDoc(docId)
  if (!doc) return
  if (!doc.official) {
    await sql`update kb_docs set okf = null where id = ${docId}`
    return
  }
  if (!doc.body.trim()) return

  // The prompt, the fallback chain, the tag parse and the failure policy all
  // live in the harness definition now (server/harness/defs/librarian.ts).
  // What survives here is the only thing that was ever this file's business:
  // turning a summary into an OKF concept and storing it.
  const run = await runHarness(librarianHarness, { title: doc.title, body: doc.body }, { caller: 'platform:librarian' })
  // FIRE AND FORGET, exactly as before: a model hiccup, an install whose gateway
  // serves nothing this harness can reach, and a reply with no usable body all
  // arrive as a null value — and the doc keeps the OKF it already had rather
  // than losing it. `onFailure: 'null'` on the definition is the other half.
  if (!run.value) return
  const { body, tags } = run.value
  // Non-null whenever a value came back (nothing here declares a fallback), but
  // the trust stamp is persisted and read by agents, so it never guesses.
  const model = run.model ?? 'unknown'
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
    `    last_modified: ${new Date(doc.updatedAt).toISOString()}`,
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

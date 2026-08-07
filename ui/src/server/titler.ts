// The Titler platform agent — names things as they take shape. Chats and
// plans get retitled once their first exchange lands (only while the title is
// still the mechanical first-message truncation — a name a user typed or
// another flow chose is never clobbered); research runs get a title from
// their question the moment they start. Everything here is fire-and-forget:
// naming must never block or fail the work it names.
import { db } from './db/pg'
import { titlerHarness, type TitleKind } from './harness/defs/titler'
import { runHarness } from './harness/run'

/** One short completion → a clean title, or null when nothing routes / the
 *  model rambles. Callers keep their existing title on null.
 *
 *  Everything this used to do by hand — resolving the model down four fallback
 *  steps, catching the upstream hiccup, taking the first non-empty line and
 *  stripping the quotes off it — is declared in harness/defs/titler.ts and done
 *  by the runner. Null still means exactly what it meant: KEEP THE CURRENT
 *  TITLE. `sweepTitles` reads it as a stop signal as well, so nothing here may
 *  ever start returning a placeholder on failure. */
export async function generateTitle(kind: TitleKind, text: string): Promise<string | null> {
  // Ahead of the harness on purpose: an empty transcript has no title in it, and
  // this early-out is what keeps a chat with no user message from spending a
  // model call and a harness_runs row to discover that.
  if (!text.trim()) return null
  return (await runHarness(titlerHarness, { kind, text }, { caller: 'platform:titler' })).value
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
  // THE GATE IS CHECKED AGAINST A SNAPSHOT AND THE WRITE HAPPENS SECONDS LATER,
  // so the write repeats the gate. `stillMechanical` above was true when the row
  // was read; a model call sits between that and here, and a rename landing in
  // the gap was silently overwritten — permanently, because a model-written
  // title no longer matches the mechanical truncation and neither retitle path
  // ever revisits the row. `is not distinct from` because the mechanical state
  // includes a NULL title.
  if (title) await sql`update conversations set title = ${title} where id = ${conversationId} and title is not distinct from ${conv.title}`
}

// ── The sweep: retroactive + ongoing naming ─────────────────────────────────
// Anything that predates the Titler (or whose naming call failed) gets picked
// up here: research runs with no title, and live conversations still wearing
// the mechanical truncation. Batched per pass so one sweep never burns much;
// failures simply wait for the next pass. Mirrors maybeSweepIdleChats.
const SWEEP_LLM_BUDGET = 12

export async function sweepTitles(): Promise<number> {
  const sql = await db()
  let spent = 0

  const runs = (await sql`
    select id, question from research_runs where title is null
    order by created_at desc limit ${SWEEP_LLM_BUDGET}
  `) as unknown as Array<{ id: string; question: string }>
  for (const r of runs) {
    if (spent >= SWEEP_LLM_BUDGET) break
    spent++
    const t = await generateTitle('research', r.question)
    if (!t) return spent // model down/rate-limited — stop burning the batch, next pass retries
    await sql`update research_runs set title = ${t} where id = ${r.id}`
  }

  // Live conversations whose title still equals the truncated first user
  // message (or the bare 'chat' fallback) — i.e., nobody named them yet.
  const convs = (await sql`
    select c.id, c.title, c.kind,
           (select m.content from messages m
            where m.conversation_id = c.id and m.role = 'user' and m.content <> ''
            order by m.seq asc limit 1) as first
    from conversations c
    where c.archived = false
      and exists (select 1 from messages m2 where m2.conversation_id = c.id and m2.role = 'assistant' and m2.content <> '')
    order by c.updated_at desc limit 100
  `) as unknown as Array<{ id: string; title: string | null; kind: 'chat' | 'plan'; first: string | null }>
  for (const c of convs) {
    if (spent >= SWEEP_LLM_BUDGET) break
    const mechanical = !c.title || c.title === 'chat' || (c.first != null && c.title === mechanicalFrom(c.first))
    if (!mechanical) continue
    spent++
    if (!(await maybeRetitleConversationAnyLength(c.id, c.kind, c.title))) return spent // ditto
  }
  return spent
}

/** Sweep-side retitle: same gate, but without the first-messages-only limit —
 *  a pre-Titler conversation can be long and still mechanically titled.
 *
 *  `expect` is the title the sweep's gate approved, and the write asserts it is
 *  still there. The sweep snapshots up to 100 rows and then makes up to twelve
 *  sequential model calls against them, so the gap between deciding a title is
 *  mechanical and overwriting it is minutes wide — and every one of those rows
 *  is renameable from the same screen that kicked the sweep. */
async function maybeRetitleConversationAnyLength(conversationId: string, kind: 'chat' | 'plan', expect: string | null): Promise<boolean> {
  const sql = await db()
  const msgs = (await sql`
    select role, content from messages
    where conversation_id = ${conversationId} and content <> '' order by seq asc limit 6
  `) as unknown as Array<{ role: string; content: string }>
  const transcript = msgs.map((m) => `${m.role}: ${m.content.slice(0, 1200)}`).join('\n\n')
  const title = await generateTitle(kind === 'plan' ? 'plan' : 'chat', transcript)
  if (title) await sql`update conversations set title = ${title} where id = ${conversationId} and title is not distinct from ${expect}`
  return title != null
}

// Opportunistic scheduling: any comms read may kick a sweep, hourly, detached.
let lastTitleSweep = 0
export function maybeSweepTitles(): void {
  const now = Date.now()
  if (now - lastTitleSweep < 60 * 60_000) return
  lastTitleSweep = now
  void sweepTitles().catch(() => {})
}

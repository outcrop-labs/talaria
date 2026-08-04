// Capability gaps — the honesty loop's memory. The contract with agents:
// competence first, and when work genuinely can't be done properly (missing
// tools/access, org-specific process the agent would be guessing at), report
// the gap instead of improvising. The contract with humans: no nagging —
// one row per work-shape ever, repeats bump seen_count (frequency is ranking
// signal), and the Studio's Suggested queue is where a gap gets ratified.
//
// ONE notification, on the FIRST sighting of a work-shape, and never again for
// that shape. That is not a softening of "no inbox pings" — it is the same
// promise, kept: the rule was always one-per-shape, and a queue that nothing
// ever announces is a queue nobody opens. The `gap_reported` class defaults to
// in-app (lib/notifications.ts), so the bell learns about a NEW kind of gap and
// no mail leaves the building unless someone asks for it. Repeats — the
// seen_count bumps that make a shape rank — say nothing at all.
import { db } from './db/pg'
import { addNotification } from './notifications'
import { audienceFor, type Authority } from './approvals'

export interface CapabilityGap {
  id: string
  kind: string
  boardId: string | null
  agentModel: string
  missing: string
  needs: string
  exampleTaskId: string | null
  seenCount: number
  status: 'open' | 'dismissed' | 'resolved'
  createdAt: string
  lastSeen: string
}

const ROW = `id, kind, board_id as "boardId", agent_model as "agentModel", missing, needs,
  example_task_id as "exampleTaskId", seen_count as "seenCount", status,
  created_at as "createdAt", last_seen as "lastSeen"`

const slug = (v: string) =>
  v.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60) || 'unclassified'

/** Work-shape identity: the board plus the agent's own name for the kind of
 *  work. Same shape reported again — from any agent — lands on the same row. */
const signatureOf = (boardId: string | null, kind: string) => `${boardId ?? 'any'}|${slug(kind)}`

// ── WHAT AN AGENT'S OWN WORDS MAY BE DISCLOSED AS ────────────────────────────
//
// THE RETRY. Refusing a `taskId` does not take the taskId away from the agent —
// it takes away the BINDING that taskId supplied, and leaves the agent holding
// the same free text with a wider default. PROVEN on this branch: an agent
// refused for a ticket re-sent the identical report with `taskId` removed,
// `boardId` resolved null, `signatureOf(null, kind)` is a different work-shape
// so the row counted as first-seen, and `{ by: 'admin', onBoard: null }` pushed
// the free text — verbatim about that board's invoices — to every admin in the
// workspace, including one with no membership of that board.
//
// Deleting the sentence that TAUGHT the retry (it was in the 403 body, and it
// was still in the MCP tool description every agent reads BEFORE it calls)
// removes the instruction and not the capability. So the capability is closed
// here, once, for every agent-raised subject — a capability gap and an agent
// problem are the same shape and must not answer this question twice.
//
// THE RULE. A refusal is REMEMBERED, briefly, against the agent that was
// refused. An unbounded report from an agent with a live refusal is therefore
// not read as an org-wide claim: it is a report about the ticket that agent was
// just told no about, and it is announced with that ticket's board's authority
// — or, when we could not place the ticket at all, with no authority, so every
// admin hears that it exists and none of them is sent the agent's words.
//
// WHY CORRELATION AND NOT REACH. The alternative weighed was to bound an
// unbounded gap by the boards the agent can work — "an agent that can only see
// board A cannot be quoting board B". It fails on the commonest refusal there
// is: a signed-off or archived ticket is refused on a board the agent IS
// allowed on, so its reach still contains that board and the bound is a no-op.
// It is also toothless in practice, because `allow_all_agents` boards are the
// default and make an agent's reach the whole workspace. And it taxes the one
// case that must keep working: a board-scoped agent reporting "I cannot send
// email at all" is making a claim about the WORKSPACE, not about its boards,
// and every admin should get those words. Correlation touches only the agent
// that was just told no, and only for the next half hour.

const REFUSAL_KEY = 'agent_ticket_refusals'
const REFUSAL_TTL_MS = 30 * 60 * 1000

/** A refusal remembered against ONE agent. `boardId` is the board we are
 *  willing to bind that agent's next unbounded report to, and it is set only
 *  when the agent is genuinely allowed on that board (a ticket a person took
 *  off the table). A ticket on a board the agent may not work — or one that
 *  does not exist — leaves it null: binding to a board the agent cannot see
 *  would let it aim its own text at a chosen board's admins, and "an agent that
 *  can only see board A cannot be quoting board B" cuts both ways. */
interface TicketRefusal {
  boardId: string | null
  at: string
}

/** Remember that this agent was just refused a ticket.
 *
 *  Keyed by the AGENT and by nothing else. Keying it by the agent plus the
 *  subject (the gap's `kind` slug, the problem's summary) would be walked past
 *  by an agent that varied one character between the refusal and the retry, and
 *  buys nothing: the only cost of the coarse key is that an agent told no about
 *  a ticket gets the quieter announcement for its next half hour of reports,
 *  and the quieter announcement still reaches every admin.
 *
 *  Never throws. This runs on the way to a 403 and must not turn a refusal into
 *  a 500 — but note that a write that fails FAILS OPEN, which is why the read
 *  side fails closed.
 *
 *  Stale entries are pruned in the same statement, so the blob cannot grow past
 *  the agents seen in the last half hour. The merge is `||` on jsonb for the
 *  reason `approvals.markAnnounced` argues at length: two refusals in flight at
 *  once must not erase each other, and a read-modify-write over a whole blob is
 *  exactly how they would. */
export async function rememberTicketRefusal(agentModel: string, boardId: string | null): Promise<void> {
  const entry = { [agentModel]: { boardId, at: new Date().toISOString() } satisfies TicketRefusal }
  const cutoff = new Date(Date.now() - REFUSAL_TTL_MS).toISOString()
  try {
    const sql = await db()
    await sql`
      insert into app_settings (key, value) values (${REFUSAL_KEY}, ${sql.json(entry as never)}::jsonb)
      on conflict (key) do update set
        value = coalesce((
          select jsonb_object_agg(e.key, e.value)
          from jsonb_each(app_settings.value) as e
          where (e.value ->> 'at') >= ${cutoff}
        ), '{}'::jsonb) || ${sql.json(entry as never)}::jsonb,
        updated_at = now()
    `
  } catch (e) {
    console.error(`[gaps] could not remember the ticket refusal for "${agentModel}":`, e)
  }
}

/** THE authority an agent's own free text may be announced under.
 *
 *  Both agent-raised subjects ask this — `reportGap` below and
 *  `routes/api/agent.problem.ts` — so the answer cannot differ between them,
 *  and neither of them decides it at the call site. The result goes straight to
 *  `audienceFor`, which is the only thing in the product that turns an authority
 *  into people.
 *
 *  · a ticket the agent WAS allowed to name → that ticket's board. Unchanged,
 *    and the honest path.
 *  · no ticket, no live refusal → `{ by: 'admin' }`. The genuinely org-wide
 *    claim, and every admin gets the words. This is the case the whole design
 *    protects: "I cannot send email at all" is not about a board.
 *  · no ticket, a live refusal we could place → that board. The retry lands
 *    exactly where the honest report would have landed, which also means it
 *    shares the honest report's work-shape signature and stops being a new gap.
 *  · no ticket, a live refusal we could NOT place → `{ by: 'nobody' }`, which
 *    `audienceFor` resolves to an empty `content` and every admin in `fact`.
 *    The agent has demonstrated this report is about a ticket, so it is not an
 *    org-wide claim; we cannot name the board, so nobody is sent the words.
 *
 *  FAILS CLOSED. A memo we could not READ is not evidence that there was no
 *  refusal, and a disclosure decision made from state we could not read must
 *  not resolve to the widest option available. So an unreadable memo is
 *  `{ by: 'nobody' }`: every admin still learns the report exists, and the words
 *  are still in the Studio for anyone who opens it. */
export async function agentTextAuthority(agentModel: string, boardId: string | null): Promise<Authority> {
  if (boardId) return { by: 'admin', onBoard: boardId }
  let memo: TicketRefusal | null
  try {
    const sql = await db()
    const rows = (await sql`
      select value -> ${agentModel} as memo from app_settings where key = ${REFUSAL_KEY}
    `) as unknown as Array<{ memo: TicketRefusal | null }>
    memo = rows[0]?.memo ?? null
  } catch (e) {
    console.error(`[gaps] could not read the refusal memo for "${agentModel}" — announcing the fact only:`, e)
    return { by: 'nobody' }
  }
  const at = memo ? Date.parse(memo.at) : NaN
  if (!Number.isFinite(at) || at < Date.now() - REFUSAL_TTL_MS) return { by: 'admin' }
  return memo!.boardId ? { by: 'admin', onBoard: memo!.boardId } : { by: 'nobody' }
}

export async function reportGap(input: {
  agentModel: string
  kind: string
  missing: string
  needs?: string
  boardId?: string | null
  taskId?: string | null
}): Promise<{ id: string; seenCount: number; first: boolean }> {
  const sql = await db()
  // The authority decides the ROW as well as the announcement. A gap the agent
  // could not bind itself, that we placed from a live refusal, gets that board's
  // signature — so it collapses onto the row the honest report would have
  // written instead of counting as a brand-new work-shape, and a retry after a
  // refusal announces nothing at all the second time.
  const authority = await agentTextAuthority(input.agentModel, input.boardId ?? null)
  const boardId = authority.by === 'admin' ? (authority.onBoard ?? null) : null
  const sig = signatureOf(boardId, input.kind)
  const rows = (await sql`
    insert into capability_gaps (signature, kind, board_id, agent_model, missing, needs, example_task_id)
    values (${sig}, ${slug(input.kind)}, ${boardId}, ${input.agentModel},
            ${input.missing.slice(0, 300)}, ${input.needs?.slice(0, 5000) ?? ''}, ${input.taskId ?? null})
    on conflict (signature) do update set
      seen_count = capability_gaps.seen_count + 1,
      last_seen = now(),
      -- a dismissed shape that keeps recurring reopens; resolved stays resolved
      status = case when capability_gaps.status = 'dismissed' then 'open' else capability_gaps.status end,
      example_task_id = coalesce(capability_gaps.example_task_id, excluded.example_task_id)
    returning id, seen_count as "seenCount", (seen_count = 1) as first
  `) as unknown as Array<{ id: string; seenCount: number; first: boolean }>
  const gap = rows[0]!
  if (gap.first) await announceGap({ ...input, authority })
  return gap
}

/** Tell the admins a NEW kind of gap exists — and tell only the ones who can see
 *  the work it quotes what it actually SAYS.
 *
 *  WHO, and how much, is `audienceFor` in server/approvals.ts: the one answer to
 *  "who should be told about this thing" in the product, which the approvals
 *  census, the judge's QA escalation and this line all resolve through.
 *
 *  The authority is `{ by: 'admin', onBoard }`. Admin, because ratifying a gap
 *  is what the Studio does with it — granting a tool, writing a skill, changing
 *  a process — and none of that is a board member's to do. Bounded by the board,
 *  because `missing` and `needs` are the AGENT'S OWN FREE TEXT, typed while
 *  working one board's ticket, and they routinely quote it: the file, the
 *  customer, the ticket's subject. What this replaced selected the admin list in
 *  raw SQL and mailed 160 characters of `missing` and 800 of `needs` to every
 *  admin in the workspace, board membership or not.
 *
 *  So the admins who can see that board get the gap. Every other admin gets the
 *  FACT — a new kind of gap exists and is waiting to be ratified — carrying not
 *  one word the agent typed. A gap with no board is org-wide and every admin
 *  gets all of it — UNLESS the agent was just refused a ticket, which is what
 *  `agentTextAuthority` above decides. This function does not re-decide it: it
 *  is handed an authority and asks the resolver, which is the whole point.
 *
 *  KNOWN RESIDUAL: the Studio's Suggested queue itself (`listGaps` below) is not
 *  board-scoped, so an admin who follows the link can still read the text. That
 *  is a PULL surface an admin chose to open, not a push into their inbox and
 *  their mail; scoping it changes `listGaps`' callers and is owned by a later
 *  round. The link stays because ratifying is the recipient's actual job.
 *
 *  The kind IS the class (`gap_reported`), which `notifyClassOf` accepts
 *  directly, so the "Capability gaps" control in Settings governs this line.
 *  Until this call site existed that control configured nothing at all.
 *
 *  Never throws: an agent reported a gap honestly and the row is already
 *  written. Losing the notification is a bad day; losing the row — or failing
 *  the agent's POST — would teach the fleet that honesty costs it something. */
async function announceGap(input: {
  agentModel: string
  kind: string
  missing: string
  needs?: string
  authority: Authority
}): Promise<void> {
  try {
    const who = await audienceFor(input.authority)
    const placed = input.authority.by === 'admin' && !!input.authority.onBoard
    for (const userId of who.content) {
      await addNotification(userId, {
        kind: 'gap_reported',
        title: `${input.agentModel} hit a capability gap: ${input.missing.slice(0, 160)}`,
        body:
          `Reported while doing ${input.kind} work.` +
          (input.needs?.trim() ? `\n\nWhat it needs: ${input.needs.trim().slice(0, 800)}` : '') +
          `\n\nRatify it in the Studio's Suggested queue, or dismiss it there.`,
        href: '/studio',
      }).catch((e: unknown) => console.error(`[gaps] could not notify ${userId} of a new gap:`, e))
    }
    for (const userId of who.fact) {
      await addNotification(userId, {
        kind: 'gap_reported',
        title: `${input.agentModel} reported a new kind of capability gap`,
        body:
          (placed
            ? 'It was raised while working a board you are not a member of, so what the agent wrote is not repeated here.'
            : 'It was raised against a ticket the agent was refused, so it is not an org-wide report and what the agent wrote is not repeated here.') +
          "\n\nRatify it in the Studio's Suggested queue, or dismiss it there.",
        href: '/studio',
      }).catch((e: unknown) => console.error(`[gaps] could not notify ${userId} that a new gap exists:`, e))
    }
  } catch (e) {
    console.error('[gaps] could not announce a new capability gap:', e)
  }
}

export async function listGaps(status?: string): Promise<CapabilityGap[]> {
  const sql = await db()
  if (status) {
    return (await sql.unsafe(
      `select ${ROW} from capability_gaps where status = $1 order by seen_count desc, last_seen desc limit 100`,
      [status],
    )) as unknown as CapabilityGap[]
  }
  return (await sql.unsafe(`select ${ROW} from capability_gaps order by seen_count desc, last_seen desc limit 100`)) as unknown as CapabilityGap[]
}

export async function setGapStatus(id: string, status: 'open' | 'dismissed' | 'resolved'): Promise<void> {
  const sql = await db()
  await sql`update capability_gaps set status = ${status} where id = ${id}`
}

export async function openGapCount(): Promise<number> {
  const sql = await db()
  const rows = (await sql`select count(*)::int as count from capability_gaps where status = 'open'`) as unknown as Array<{ count: number }>
  return rows[0]?.count ?? 0
}

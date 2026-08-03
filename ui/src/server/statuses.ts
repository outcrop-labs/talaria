// Board statuses — per-board workflow columns with SEMANTICS, not just names.
//   category: open (intake) | active (working) | review (agent-review catch) | done (terminal)
//   agentStart: columns that constitute assignment approval — the agent
//               heartbeat only picks up work sitting in one of these.
// BLOCKED is a system status: always present, always the same, never stored.
// Boards without rows get the shipped default set (identical to the classic
// fixed statuses), so nothing changes until a board customizes.
import { db } from './db/pg'
import { publishBoard } from './realtime'

export type StatusCategory = 'open' | 'active' | 'review' | 'done'

/** Legal on every board but never board COLUMNS, so they carry no category —
 *  terminal in every UI that reads them, and agents may not park work there.
 *  Lives HERE, next to the resolvers that must exclude them, because a board
 *  CAN mint a column keyed 'failed' (slug of "Failed") and every resolver that
 *  hands an agent a destination has to agree with the engine about what those
 *  two words mean. server/tasks.ts imports it rather than re-declaring it. */
export const OFF_BOARD_STATUSES = ['failed', 'cancelled']

export interface BoardStatus {
  id: string | null // null for virtual defaults + the system blocked row
  key: string
  label: string
  color: string
  category: StatusCategory | 'blocked'
  agentStart: boolean
  position: number
  system?: boolean
}

export const BLOCKED: BoardStatus = {
  id: null,
  key: 'blocked',
  label: 'Blocked',
  color: 'red',
  category: 'blocked',
  agentStart: false,
  position: 0,
  system: true,
}

const DEFAULTS: Omit<BoardStatus, 'position'>[] = [
  { id: null, key: 'inbox', label: 'Inbox', color: 'slate', category: 'open', agentStart: false },
  { id: null, key: 'assigned', label: 'Assigned', color: 'bronze', category: 'open', agentStart: true },
  { id: null, key: 'in_progress', label: 'In progress', color: 'amber', category: 'active', agentStart: true },
  { id: null, key: 'quality_review', label: 'Quality review', color: 'purple', category: 'review', agentStart: false },
  { id: null, key: 'done', label: 'Done', color: 'green', category: 'done', agentStart: false },
]

const ROW = `id, key, label, color, category, agent_start as "agentStart", position`

/** The board's ordered status list, BLOCKED injected after the last
 *  active-category status (or mid-list for pure defaults). */
export async function listStatuses(boardId: string): Promise<BoardStatus[]> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${ROW} from board_statuses where board_id = $1 order by position, created_at`, [
    boardId,
  ])) as unknown as BoardStatus[]
  const base = rows.length
    ? rows
    : DEFAULTS.map((d, i) => ({ ...d, position: i }))
  // Inject the system blocked column after the last 'active' status (falls
  // back to just-before-review, then end).
  let at = -1
  for (let i = 0; i < base.length; i++) if (base[i]!.category === 'active') at = i
  if (at === -1) {
    const r = base.findIndex((s) => s.category === 'review')
    at = r === -1 ? base.length - 1 : r - 1
  }
  const out = [...base]
  out.splice(at + 1, 0, { ...BLOCKED, position: at + 1 })
  return out.map((s, i) => ({ ...s, position: i }))
}

/** Workflow metadata the task engine needs, resolved per board.
 *
 *  NO KEY HERE IS A GUESS. Every field either names a column this board really
 *  has, or is empty/null. The one exception is `doneKeys`, whose fallback is
 *  documented below and is fail-SAFE (it only ever widens what counts as
 *  closed, i.e. what agents may not touch). Everything else used to fall back
 *  to a bare literal, and a literal that may or may not correspond to a real
 *  column of the right category is how an agent's hand-off got laundered into
 *  a done column: `reviewKey` resolved to 'quality_review' on a board where
 *  'quality_review' had been recategorised to `done`, so "report a result"
 *  wrote "closed, signed off". Resolvers do not invent destinations — callers
 *  that find nothing must refuse. */
export async function statusMeta(boardId: string): Promise<{
  keys: string[]
  agentStartKeys: string[]
  /** Where an agent's terminal move is redirected: the first review-category
   *  column a PERSON can actually sign off from — so never one that is itself
   *  an agent-start pickup queue, which would just hand the work back. Null
   *  when the board has no such column: there is nowhere to hand off, and the
   *  caller must say so rather than pick something. */
  reviewKey: string | null
  /** Every review-category column on the board (possibly empty). */
  reviewKeys: string[]
  doneKeys: string[]
  /** The board's intake column — where a new, unassigned ticket lands. Null
   *  when the board has no open-category column (illegal now that
   *  REQUIRED_CATEGORIES protects the last one, but reachable in legacy data);
   *  the caller says so rather than dropping the ticket somewhere. */
  defaultKey: string | null
  /** The board's PICKUP QUEUE — the column that constitutes assignment
   *  approval. Same rule as reviewKey, and for the same reason: `agentStart[0]
   *  ?? open` INVENTED one. `agent_start` on a done-category column was
   *  accepted (createStatus/updateStatus refuse it now, legacy rows survive),
   *  and REQUIRED_CATEGORIES protects open/review/done but NOT `active`, so a
   *  board can legally reach zero active columns — between them, "assigned"
   *  resolved to a DONE column and creating or promoting a ticket closed it.
   *  So: a real agent-start column that is not terminal (done or off-board) and
   *  not the reviewer's sign-off queue, or null. Callers refuse or stay put. */
  assignedKey: string | null
}> {
  const list = await listStatuses(boardId)
  const agentStart = list.filter((s) => s.agentStart).map((s) => s.key)
  const reviews = list.filter((s) => s.category === 'review').map((s) => s.key)
  const done = list.filter((s) => s.category === 'done').map((s) => s.key)
  // Fail-SAFE fallback, unlike the review one: doneKeys is only ever read to
  // decide what an agent may NOT do (a closed ticket is untouchable, a terminal
  // move gets redirected), so a legacy board that carries tickets at the
  // shipped 'done' key without a matching column still treats them as closed.
  // assertNotLastOfCategory() keeps boards out of that state now.
  const doneKeys = done.length ? done : ['done']
  const terminal = (k: string) => doneKeys.includes(k) || OFF_BOARD_STATUSES.includes(k)
  return {
    keys: list.map((s) => s.key),
    agentStartKeys: agentStart,
    reviewKey: reviews.find((k) => !agentStart.includes(k) && !terminal(k)) ?? null,
    reviewKeys: reviews,
    doneKeys,
    // Never the system Blocked column (listStatuses injects it, so it can be
    // list[0]) and never a terminal or sign-off column: an intake that resolves
    // to one of those places new work somewhere nobody chose.
    defaultKey: list.find((s) => s.category === 'open')?.key ?? null,
    assignedKey: agentStart.find((k) => !terminal(k) && !reviews.includes(k)) ?? null,
  }
}

/** SQL fragment: `t.status` is in the given CATEGORY on the ticket's own
 *  board, with the legacy fallback for never-customized boards. Interpolate
 *  only with a literal category + fallback list (no user input). */
export const statusCategorySql = (category: StatusCategory, legacyKeys: string[]): string => `(
  t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = '${category}')
  or (
    not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
    and t.status in (${legacyKeys.map((k) => `'${k}'`).join(', ')})
  )
)`

/** First customization COPIES the defaults into rows so edits are complete. */
async function materialize(boardId: string): Promise<void> {
  const sql = await db()
  const existing = await sql`select 1 from board_statuses where board_id = ${boardId} limit 1`
  if (existing.length) return
  for (let i = 0; i < DEFAULTS.length; i++) {
    const d = DEFAULTS[i]!
    await sql`
      insert into board_statuses (board_id, key, label, color, category, agent_start, position)
      values (${boardId}, ${d.key}, ${d.label}, ${d.color}, ${d.category}, ${d.agentStart}, ${i})
      on conflict do nothing
    `
  }
}

const slug = (label: string) =>
  label.trim().toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '').slice(0, 40) || 'status'

// ── Structural invariants ────────────────────────────────────────────────────
// statusMeta RESOLVES a workflow position (intake / review catch / terminal) to
// a column key, and the task engine writes whatever it resolves. So the honest
// place to guarantee the resolution finds something real is HERE, on the two
// operations that can take a position away — and they have to be symmetric.
// deleteStatus protected the last intake and the last review catch; updateStatus
// did not, so `PUT {statusKey:'quality_review', category:'done'}` walked a board
// to zero review columns through the ordinary owner-facing route, and the
// hand-off destination then resolved to a DONE column. Recategorising is the
// same loss as deleting — same rule.

/** The workflow positions a board cannot do without, and the word the refusal
 *  uses for each. */
const REQUIRED_CATEGORIES: Record<string, string> = { open: 'intake', review: 'review', done: 'terminal' }

/** Refuse an operation that would leave this board with no column of `category`
 *  (the row being deleted, or recategorised away, is `exceptId`). */
async function assertNotLastOfCategory(boardId: string, exceptId: string, category: string): Promise<void> {
  const noun = REQUIRED_CATEGORIES[category]
  if (!noun) return
  const sql = await db()
  const others = await sql`
    select 1 from board_statuses where board_id = ${boardId} and category = ${category} and id <> ${exceptId} limit 1
  `
  if (!others.length) throw new Error(`the workflow needs at least one ${noun} status — this is the board's last one`)
}

/** Categories whose columns are HUMAN GATES: work parked there is waiting for,
 *  or already has, a person's sign-off. The word each refusal uses. */
const SIGNOFF_CATEGORIES: Record<string, string> = { review: 'waiting for sign-off', done: 'a person signed off on' }

/** Refuse recategorising a sign-off column that still HOLDS TICKETS.
 *
 *  The review+agentStart pair is checked against the EFFECTIVE post-patch
 *  column, which is correct per write — and precisely why it could be split
 *  across two: `PUT {quality_review, category:'active'}` then
 *  `PUT {quality_review, agentStart:true}` both pass, and the tickets sitting
 *  in that column awaiting a person's sign-off are silently released — re-served
 *  by assignedWork, freely movable by the agent (the review-exit rule reads the
 *  CATEGORY, which no longer says review), with no activity line anywhere saying
 *  they left review. The column's flags are board config; the tickets in it are
 *  not, and moving them is a decision a person makes ticket by ticket. So the
 *  flip is refused while anyone is parked there — empty the column first (drag
 *  the tickets, or delete the column with `reassignTo`, which moves them
 *  explicitly). */
async function assertSignoffColumnEmpty(boardId: string, key: string, from: string, to: string): Promise<void> {
  if (from === to) return
  const what = SIGNOFF_CATEGORIES[from]
  if (!what) return
  const sql = await db()
  const [row] = (await sql`
    select count(*)::int as n from tasks where board_id = ${boardId} and status = ${key} and archived_at is null
  `) as unknown as Array<{ n: number }>
  const n = row?.n ?? 0
  if (!n) return
  throw new Error(
    `"${key}" still holds ${n} ticket${n === 1 ? '' : 's'} ${what} — recategorising it to ${to} would release ${n === 1 ? 'it' : 'them'} back to the agents with nothing on the record saying they left ${from}. Move ${n === 1 ? 'that ticket' : 'those tickets'} to another ${from} column first (or delete this column and reassign them).`,
  )
}

/** `review` and `agentStart` are opposite ends of the same handover: a review
 *  column is the queue a PERSON signs work off from, an agent-start column is
 *  the queue agents pick work UP from. Flagged both, an agent's hand-off drops
 *  straight back into its own pickup queue. The API route validates the
 *  EFFECTIVE post-patch column before it gets here; this is the invariant
 *  itself, so any future caller inherits it too. */
export const REVIEW_AGENT_START_CONFLICT =
  'a review column is where a person signs work off — it cannot also be an agent-start column agents pick work up from'

/** The sibling rule, and it was missing: `agent_start` on a DONE column turns
 *  CLOSING a ticket into a dispatch. One legal
 *  `PUT {statusKey:'done', agentStart:true}` was accepted; a human then moving an
 *  agent-assigned ticket to 'done' hit updateTask's re-dispatch branch, which
 *  wrote a 'dispatch' activity row and started a live work session ON THE CLOSED
 *  TICKET. Terminal and pickup are opposite ends of the same handover, exactly
 *  like review and pickup. */
export const DONE_AGENT_START_CONFLICT =
  'a done column is where work is closed — it cannot also be an agent-start column agents pick work up from, or closing a ticket would dispatch fresh work on it'

/** The one rule, stated once, over the EFFECTIVE post-patch column: an
 *  agent-start pickup queue is neither a human gate. Returns the refusal, or
 *  null when the pair is fine. */
export function agentStartConflict(category: StatusCategory, agentStart: boolean): string | null {
  if (!agentStart) return null
  if (category === 'review') return REVIEW_AGENT_START_CONFLICT
  if (category === 'done') return DONE_AGENT_START_CONFLICT
  return null
}

export async function createStatus(
  boardId: string,
  input: { label: string; color?: string; category?: StatusCategory; agentStart?: boolean },
): Promise<BoardStatus> {
  await materialize(boardId)
  const conflict = agentStartConflict(input.category ?? 'active', input.agentStart ?? false)
  if (conflict) throw new Error(conflict)
  const sql = await db()
  let key = slug(input.label)
  if (key === 'blocked') key = 'blocked_2' // the system column owns the name
  const clash = await sql`select 1 from board_statuses where board_id = ${boardId} and key = ${key}`
  if (clash.length) key = `${key}_${Math.random().toString(36).slice(2, 6)}`
  const rows = (await sql`
    insert into board_statuses (board_id, key, label, color, category, agent_start, position)
    values (${boardId}, ${key}, ${input.label.trim()}, ${input.color ?? 'slate'}, ${input.category ?? 'active'},
            ${input.agentStart ?? false},
            coalesce((select max(position) + 1 from board_statuses where board_id = ${boardId}), 0))
    returning ${sql.unsafe(ROW)}
  `) as unknown as BoardStatus[]
  publishBoard(boardId, { type: 'board' })
  return rows[0]!
}

/** Key-addressed: boards that never customized serve VIRTUAL defaults with
 *  no row ids, so the stable handle is the status KEY — materialize() turns
 *  the virtual set into rows on first touch, then the key resolves. */
export async function updateStatus(
  boardId: string,
  key: string,
  patch: { label?: string; color?: string; category?: StatusCategory; agentStart?: boolean; position?: number },
): Promise<void> {
  if (key === 'blocked') throw new Error('Blocked is a system status')
  await materialize(boardId)
  const sql = await db()
  const [cur] = (await sql`
    select id, category, agent_start as "agentStart" from board_statuses where key = ${key} and board_id = ${boardId}
  `) as unknown as Array<{ id: string; category: string; agentStart: boolean }>
  // A key that names nothing used to no-op and report success — a silent
  // failure, and one that hides a typo'd guard rather than surfacing it.
  if (!cur) throw new Error(`"${key}" is not a status on this board`)
  // Recategorising is the same loss as deleting: symmetric with deleteStatus.
  if (patch.category !== undefined && patch.category !== cur.category) {
    await assertNotLastOfCategory(boardId, cur.id, cur.category)
    // …and it is the same release as moving every ticket out by hand.
    await assertSignoffColumnEmpty(boardId, key, cur.category, patch.category)
  }
  // The effective post-patch column, so neither flag can arrive alone and slip
  // the pair past the check.
  const conflict = agentStartConflict((patch.category ?? cur.category) as StatusCategory, patch.agentStart ?? cur.agentStart)
  if (conflict) throw new Error(conflict)
  if (patch.label !== undefined) await sql`update board_statuses set label = ${patch.label.trim()} where key = ${key} and board_id = ${boardId}`
  if (patch.color !== undefined) await sql`update board_statuses set color = ${patch.color} where key = ${key} and board_id = ${boardId}`
  if (patch.category !== undefined) await sql`update board_statuses set category = ${patch.category} where key = ${key} and board_id = ${boardId}`
  if (patch.agentStart !== undefined) await sql`update board_statuses set agent_start = ${patch.agentStart} where key = ${key} and board_id = ${boardId}`
  if (patch.position !== undefined) await sql`update board_statuses set position = ${patch.position} where key = ${key} and board_id = ${boardId}`
  publishBoard(boardId, { type: 'board' })
}

/** Replace the whole order (array of status KEYS in the new order). */
export async function reorderStatuses(boardId: string, keys: string[]): Promise<void> {
  await materialize(boardId)
  const sql = await db()
  for (let i = 0; i < keys.length; i++) {
    await sql`update board_statuses set position = ${i} where key = ${keys[i]!} and board_id = ${boardId}`
  }
  publishBoard(boardId, { type: 'board' })
}

/** Delete a status; its tickets move to `reassignTo` (a status key). The last
 *  intake, the last review catch and the last terminal column are protected —
 *  the workflow needs all three, and statusMeta resolves each of them to a real
 *  key. Same rule as updateStatus (see REQUIRED_CATEGORIES). */
export async function deleteStatus(boardId: string, key: string, reassignTo: string): Promise<void> {
  if (key === 'blocked') throw new Error('Blocked is a system status')
  await materialize(boardId)
  const sql = await db()
  const [victim] = (await sql`select id, key, category from board_statuses where key = ${key} and board_id = ${boardId}`) as unknown as Array<{
    id: string
    key: string
    category: string
  }>
  if (!victim) return
  await assertNotLastOfCategory(boardId, victim.id, victim.category)
  const meta = await statusMeta(boardId)
  if (!meta.keys.includes(reassignTo) || reassignTo === victim.key) throw new Error('pick a surviving status for its tickets')
  await sql`update tasks set status = ${reassignTo}, updated_at = now() where board_id = ${boardId} and status = ${victim.key}`
  await sql`delete from board_statuses where id = ${victim.id}`
  publishBoard(boardId, { type: 'board' })
}

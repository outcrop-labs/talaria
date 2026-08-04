// Board statuses — per-board workflow columns with SEMANTICS, not just names.
//   category: open (intake) | active (working) | review (agent-review catch) | done (terminal)
//   agentStart: columns that constitute assignment approval — the agent
//               heartbeat only picks up work sitting in one of these.
// BLOCKED is a system status: always present, always the same, never stored.
// Boards without rows get the shipped default set (identical to the classic
// fixed statuses), so nothing changes until a board customizes.
import { db } from './db/pg'
import { publishBoard } from './realtime'
import type { TaskStatus } from '@/lib/task-const'

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
 *  that find nothing must refuse.
 *
 *  ONE FILTERED LIST ANSWERS "WHERE MAY A TICKET BE PLACED?". `placeable` (see
 *  below) is that answer, and EVERY destination field here is picked from it.
 *  Nothing outside this file may re-derive a destination from `listStatuses` —
 *  three callers did (the dispatch prompt's "work in this column" hint, the
 *  judge's revision bounce, the human reviewer's "request changes"), each
 *  spelling `list.find(s => s.category === 'active')`, and each therefore able
 *  to hand out a column keyed `cancelled`. `activeKey` is the field they all
 *  needed. */
export interface StatusMeta {
  keys: string[]
  /** The RAW `agent_start` flag set — every column carrying it, terminal or
   *  human-gated or not. This is the REFUSAL set, and deliberately the widest
   *  one: entering any of these is assignment, which an agent may not do for
   *  itself. Never use it to pick a destination or to decide what to serve —
   *  `pickupKeys`/`workingKeys` are the filtered answers for that. Refusals
   *  take the widest set, destinations the narrowest. */
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
  /** The board's intake column — where a new, unassigned ticket lands. Never a
   *  terminal column (a done key or an off-board key): an owner may legally
   *  label an OPEN column "Cancelled", whose slug is the off-board terminal
   *  key, and a ticket born there is born closed to agents. Null when the board
   *  has no usable open-category column (illegal now that REQUIRED_CATEGORIES
   *  protects the last one, but reachable in legacy data); the caller says so
   *  rather than dropping the ticket somewhere. */
  defaultKey: string | null
  /** The board's PICKUP QUEUE — the column that constitutes assignment
   *  approval. Same rule as reviewKey, and for the same reason: `agentStart[0]
   *  ?? open` INVENTED one. `agent_start` on a done-category column was
   *  accepted (createStatus/updateStatus refuse it now, legacy rows survive),
   *  and REQUIRED_CATEGORIES protects open and done always and review on
   *  agent-permitting boards, but `active` NEVER, so a board can legally reach
   *  zero active columns — between them, "assigned"
   *  resolved to a DONE column and creating or promoting a ticket closed it.
   *  So: a real agent-start column that is not terminal (done or off-board) and
   *  not the reviewer's sign-off queue, or null. Callers refuse or stay put. */
  assignedKey: string | null
  /** Every column work may actually be PICKED UP from: agent-start, placeable,
   *  and not a review column. `assignedKey` is its first entry. A review column
   *  that also carries `agent_start` (legacy data — agentStartConflict refuses
   *  the write now) is NOT one: the review-exit rule freezes anything parked
   *  there, so dispatching into it or serving it on the heartbeat produces an
   *  agent that loops forever on work it can never move. */
  pickupKeys: string[]
  /** Every column a WORK SESSION may continue in: pickup queues plus the active
   *  (working) columns. Placeable, so never terminal and never the system
   *  Blocked column; never a review column, for the same reason as
   *  `pickupKeys`. This is the one answer to "is this ticket still in play for
   *  its agent?" — the session loop and the heartbeat both ask it. */
  workingKeys: string[]
  /** The board's first WORKING column — where an agent parks a ticket while it
   *  works, where the judge bounces a revision back to, and where a human
   *  "request changes" sends it. Placeable, so a column labelled "Cancelled"
   *  (slug: the off-board terminal key) can never be it. Null when the board
   *  has no active column at all; callers fall back to `assignedKey` or say so. */
  activeKey: string | null
  /** Is this key TERMINAL on this board — a done-category column, or one of the
   *  off-board keys that are terminal everywhere? The one definition: callers
   *  used to write `meta.doneKeys.includes(k) || OFF_BOARD_STATUSES.includes(k)`
   *  by hand, and the ones that wrote only the first half are how a pickup
   *  queue keyed `cancelled` became a dispatch target. (Not enumerable as data
   *  on purpose — a function cannot be half-copied.) */
  terminal: (key: string) => boolean
}

export async function statusMeta(boardId: string): Promise<StatusMeta> {
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
  // ── One filtered list, three positions ─────────────────────────────────────
  // reviewKey/assignedKey each called `terminal()` for themselves and
  // `defaultKey` — added later, by a different hand — did not, so a board owner
  // who created an open-category column labelled "Cancelled" got an intake that
  // resolved to the off-board terminal key: every new ticket was born CLOSED to
  // agents and nothing could triage it. Three separate calls is three chances to
  // forget, so the rule is applied ONCE, to the list all three pick from.
  // `placeable` is "a column a resolver may hand a ticket to": never terminal
  // (a done key or an off-board key), never the system Blocked column
  // (listStatuses injects it, so it can be list[0]). A fourth sibling added
  // later inherits the rule by picking from here.
  const placeable = list.filter((s) => s.category !== 'blocked' && !terminal(s.key))
  const pick = (f: (s: BoardStatus) => boolean): string | null => placeable.find(f)?.key ?? null
  const all = (f: (s: BoardStatus) => boolean): string[] => placeable.filter(f).map((s) => s.key)
  // A review column is the human sign-off queue at BOTH ends: an agent may not
  // move a ticket out of one, so a review column can never be somewhere work is
  // picked up or continued — whatever its `agent_start` flag says. Stating that
  // once, here, is what stops `assignedWork` re-serving a frozen ticket on
  // every heartbeat while `agentSafePatch` refuses every move out of it.
  const pickupKeys = all((s) => s.agentStart && s.category !== 'review')
  const workingKeys = all((s) => (s.agentStart || s.category === 'active') && s.category !== 'review')
  return {
    keys: list.map((s) => s.key),
    agentStartKeys: agentStart,
    reviewKey: pick((s) => s.category === 'review' && !s.agentStart),
    reviewKeys: reviews,
    doneKeys,
    defaultKey: pick((s) => s.category === 'open'),
    assignedKey: pickupKeys[0] ?? null,
    pickupKeys,
    workingKeys,
    activeKey: pick((s) => s.category === 'active'),
    terminal,
  }
}

/** A board-shape problem, in one sentence, addressed to the person who can fix
 *  it. `error` = something the board cannot do at all; `warning` = something it
 *  will do, silently, that is probably not what was meant. */
export interface StatusDiagnostic {
  level: 'error' | 'warning'
  text: string
}

/** WHAT THE BOARD OWNER CANNOT OTHERWISE SEE.
 *
 *  Phase 0 made every resolver refuse rather than guess — `reviewKey` is null on
 *  a board whose only review column is also an agent-start queue, and the
 *  hand-off then throws with a diagnostic that names the switch to flip. That
 *  diagnostic goes to the AGENT (as the error on its own write) and to
 *  `console.warn`. Neither is the board owner. So a board carrying a legacy
 *  `review + agentStart` row — legal before the cross-validation, still on disk,
 *  no migration — refuses every hand-off on it, forever, while the statuses tab
 *  shows a tidy list of columns and says nothing. The person who can fix it in
 *  two clicks is the one person not told.
 *
 *  This is that telling. It is DERIVED, not re-derived: every judgment below
 *  reads `statusMeta`'s resolved fields (`reviewKey`, `pickupKeys`, `defaultKey`)
 *  rather than re-asking the question from the column list, so a board is called
 *  broken here exactly when the engine will actually refuse. The route serves it
 *  beside the columns; the statuses tab renders it above them. */
export async function statusDiagnostics(boardId: string): Promise<StatusDiagnostic[]> {
  const list = await listStatuses(boardId)
  const meta = await statusMeta(boardId)
  const out: StatusDiagnostic[] = []
  const name = (key: string) => `“${list.find((s) => s.key === key)?.label ?? key}”`
  const cols = (keys: string[]) => keys.map(name).join(', ')

  // Data that predates the cross-validation, and the ONE thing an owner has to
  // do about it. `agentStartConflict` is the same rule the writes use, so this
  // panel calls a row illegal exactly when a write of it would be refused. The
  // two categories break DIFFERENT things, and a diagnostic that names the
  // wrong consequence sends the reader looking in the wrong place.
  const legacy = list.filter((s) => s.category !== 'blocked' && agentStartConflict(s.category as StatusCategory, s.agentStart))
  for (const s of legacy) {
    const consequence =
      s.category === 'review'
        ? 'so it is not a column an agent can hand work off to, and hand-offs that would land here are refused'
        : 'so moving a ticket into it fires a fresh dispatch on the work a person just closed'
    out.push({
      level: 'error',
      text: `“${s.label}” is a ${s.category} column and also flagged “agent start” — ${consequence}. That pairing is refused on new edits but survives from before the rule and there is no migration for it, so it stays until someone clears the box. Clear “agent start” on it.`,
    })
  }
  // THE AGENT-WORKFLOW DIAGNOSTICS ONLY APPLY TO BOARDS THAT RUN AGENTS. The
  // three below all describe something AGENTS cannot do here, and the panel
  // renders an error as "Agents are stuck on this board" — true and useful on a
  // board that permits agents, and a permanent complaint about a workflow the
  // owner deliberately chose on one that does not. Same predicate as the
  // last-review-column rule (AGENT_REQUIRED_CATEGORIES), so the tab is silent
  // about exactly the boards that rule now lets exist, and speaks up the moment
  // someone opens one to agents. Everything outside this block — malformed
  // rows, a missing intake column, off-board keys — is wrong for every board and
  // stays unconditional.
  if (await boardPermitsAgents(boardId)) {
    if (meta.reviewKeys.length === 0) {
      out.push({
        level: 'error',
        text: 'This board has no review column, so an agent that finishes a ticket has nowhere to hand it — it cannot close the ticket itself, and the hand-off is refused. Add a column with the category “review”, or turn agents off for this board.',
      })
    } else if (meta.reviewKey === null) {
      // Only the review columns the line above has not already accounted for —
      // otherwise a board with one legacy row says the same thing twice.
      const unexplained = meta.reviewKeys.filter((k) => !legacy.some((s) => s.key === k))
      if (unexplained.length) {
        out.push({
          level: 'error',
          text: `${cols(unexplained)} cannot receive a hand-off: a review column that is itself terminal (its key means “off the board”) is not somewhere work can be signed off from. No agent can hand work off on this board until one review column can hold it.`,
        })
      }
    }
    if (meta.pickupKeys.length === 0) {
      out.push({
        level: 'warning',
        text: 'No column is a usable agent-start queue, so agents will never pick work up on this board. Tick “agent start” on the intake or active column where assigning a ticket should count as approval to begin.',
      })
    }
  }
  if (meta.defaultKey === null) {
    out.push({
      level: 'error',
      text: 'This board has no intake column, so new tickets have nowhere to land and creating one fails. Add a column with the category “intake”.',
    })
  }
  // A column whose SLUG collides with an off-board key is terminal everywhere in
  // Talaria, whatever category the owner chose — so it silently drops out of
  // every destination the engine can resolve. Renaming does not move a key;
  // only a new column does.
  const offBoard = list.filter((s) => s.category !== 'blocked' && s.category !== 'done' && OFF_BOARD_STATUSES.includes(s.key))
  if (offBoard.length) {
    out.push({
      level: 'warning',
      text: `${cols(offBoard.map((s) => s.key))} ${offBoard.length === 1 ? 'has the key' : 'have the keys'} ${offBoard.map((s) => `“${s.key}”`).join(', ')}, which ${offBoard.length === 1 ? 'means' : 'mean'} “off the board” everywhere in Talaria — tickets there are closed to agents and nothing routes work into ${offBoard.length === 1 ? 'it' : 'them'}, whatever category ${offBoard.length === 1 ? 'it has' : 'they have'}. A column’s key is fixed at creation, so replace ${offBoard.length === 1 ? 'it' : 'them'} with a differently-named column.`,
    })
  }
  return out
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

/** …and the one of them that is required BY THE AGENTS, not by the workflow.
 *
 *  WHY REVIEW IS CONDITIONAL AND THE OTHER TWO ARE NOT. Intake and terminal are
 *  needed by every board, whoever works it: with no open column `defaultKey` is
 *  null and CREATING A TICKET FAILS (there is nowhere for it to land), and with
 *  no done column there is nowhere for a person to close one — `doneKeys` falls
 *  back to a phantom 'done' key that is not a column, so the board can be
 *  worked but never finished. Both break a board with no agents anywhere near
 *  it. Review does not: a review column is the AGENT HAND-OFF TARGET, the place
 *  an agent puts work it may not close itself. Todo / Doing / Done with no
 *  sign-off step is an ordinary, correct board for a team not running agents on
 *  it, and refusing to let them build one was a rule enforcing an agent
 *  workflow on boards that have no agents.
 *
 *  SO THE RULE FOLLOWS THE DEPENDENCY: absolute for a board that permits
 *  agents, absent for one that does not. It is safe to make conditional
 *  precisely because of what the engine does when the column is missing — it
 *  REFUSES. `handoffTarget` throws a sentence naming the fix rather than
 *  picking a destination, so the worst case of a board that later opens itself
 *  to agents is an agent that cannot finish, loudly, with `statusDiagnostics`
 *  telling the owner why in board settings. There is no path here to a
 *  hand-off laundered into a done column — that would be a wrong loosening, and
 *  this is not one. */
const AGENT_REQUIRED_CATEGORIES = new Set(['review'])

/** The refusal, for a board that DOES permit agents. It has to explain both
 *  halves — why agents need the column, and that turning agents off is the
 *  other way to get what the owner asked for. */
export const REVIEW_REQUIRED_FOR_AGENTS =
  'this is the board’s last review column, and agents are allowed to work this board. An agent may never sign off its own work, so it hands a finished ticket to a review column for a person — with none, every hand-off is refused and an agent here can start work it can never finish. Keep one review column, or turn agents off for this board (board settings → agents), after which a review column is no longer required.'

/** Does this board admit agents at all — the board's own policy, allow-all or
 *  an explicit allow-list. Dynamic import because server/boards.ts imports THIS
 *  module (`statusMeta`), so a static one would close the cycle — the same
 *  reason `deleteStatus` imports `./tasks` dynamically. */
async function boardPermitsAgents(boardId: string): Promise<boolean> {
  const { getBoardAgentConfig } = await import('./boards')
  const cfg = await getBoardAgentConfig(boardId)
  return cfg.allowAll || cfg.models.length > 0
}

/** Refuse an operation that would leave this board with no column of `category`
 *  (the row being deleted, or recategorised away, is `exceptId`). */
async function assertNotLastOfCategory(boardId: string, exceptId: string, category: string): Promise<void> {
  const noun = REQUIRED_CATEGORIES[category]
  if (!noun) return
  const sql = await db()
  const others = await sql`
    select 1 from board_statuses where board_id = ${boardId} and category = ${category} and id <> ${exceptId} limit 1
  `
  if (others.length) return
  if (AGENT_REQUIRED_CATEGORIES.has(category)) {
    if (!(await boardPermitsAgents(boardId))) return
    throw new Error(REVIEW_REQUIRED_FOR_AGENTS)
  }
  throw new Error(`the workflow needs at least one ${noun} status — this is the board's last one`)
}

/** Categories whose columns are HUMAN GATES: work parked there is waiting for,
 *  or already has, a person's sign-off. The word each message uses. */
const SIGNOFF_CATEGORIES: Record<string, string> = { review: 'waiting for sign-off', done: 'a person signed off on' }

/** Recategorising a sign-off column that still HOLDS TICKETS: move them first.
 *
 *  THE HAZARD IS THE UNRECORDED RELEASE, NOT THE RECATEGORISATION. The
 *  review+agentStart pair is checked against the EFFECTIVE post-patch column,
 *  which is correct per write — and precisely why it could be split across two:
 *  `PUT {quality_review, category:'active'}` then
 *  `PUT {quality_review, agentStart:true}` both pass, and the tickets sitting in
 *  that column awaiting a person's sign-off were silently released — re-served
 *  by assignedWork, freely movable by the agent (the review-exit rule reads the
 *  CATEGORY, which no longer says review), with no activity line anywhere saying
 *  they left review.
 *
 *  This used to REFUSE outright, and its refusal recommended "delete this column
 *  and reassign them" — a path that does strictly more work than the one being
 *  refused, and does it safely, because `deleteStatus` moves each ticket through
 *  `updateTask` (activity row, completed_at, watchers, dispatch). So the refused
 *  path now does the same thing: the held tickets move, one `updateTask` each,
 *  as the person who reshaped the column, into a SURVIVING COLUMN OF THE SAME
 *  CATEGORY — so a ticket awaiting sign-off is still awaiting sign-off
 *  afterwards, and every move is on its record. The reshape then applies.
 *
 *  `assertNotLastOfCategory` runs FIRST, so on a board that permits agents a
 *  review sibling always exists — but it is no longer the only shape here (see
 *  AGENT_REQUIRED_CATEGORIES), and `done` was never protected against reaching
 *  its last column with tickets in it either. So all three counts are handled:
 *  with exactly one sibling there is nothing to choose; with several the
 *  destination is a real decision and this REFUSES, naming them — picking "the
 *  first one" would be the invented destination this file exists to stop; with
 *  NONE there is no column of the same kind to move to at all, and the owner has
 *  to place the tickets themselves.
 *
 *  ARCHIVED TICKETS ARE DRAINED TOO, and the filter that skipped them was a
 *  hole. A ticket archived while it sat in a review column is still a ticket
 *  awaiting sign-off — archival hides it, it does not resolve it. Left behind,
 *  it stays at a key whose column has since become `active`, and the moment
 *  anyone restores it the review-exit rule no longer applies (that rule reads
 *  the CATEGORY): the agent can move it freely, which is exactly the silent
 *  release this function exists to prevent, just deferred until someone
 *  un-archives. `deleteStatus` has always moved archived tickets — the two
 *  operations are the same loss and now agree.
 *
 *  NOT A TRANSACTION, DELIBERATELY. Wrapping the loop in `sql.begin` would buy
 *  nothing and claim a lot: `updateTask` takes its own connection from the pool,
 *  so its writes would not enrol in the outer transaction at all, and half of
 *  what it does per ticket ESCAPES the database entirely — a realtime publish, a
 *  notification fan-out, a push, a work-session dispatch. A rollback cannot
 *  unsend those. Rolling back by hand (moving the moved ones back) is strictly
 *  more of the same risk plus a second round of activity rows saying the
 *  opposite. So the shape is the other one the brief allows: PARTIAL PROGRESS IS
 *  MADE SAFE, AND REPORTED.
 *    · SAFE — the destination is a surviving column OF THE SAME CATEGORY, so a
 *      ticket that has moved is in the same workflow position it was in: still
 *      awaiting sign-off, still frozen to agents by the review-exit rule. A
 *      half-drained column is a legal board, not a broken one.
 *    · ORDERED — the drain runs BEFORE the category is written, so a failure
 *      leaves the column with its original category. The half that moved is
 *      protected by the category it moved WITHIN; the half that did not is
 *      protected by the category the column still has. Neither half is released.
 *    · RESUMABLE — the moved tickets are out of this column, so re-issuing the
 *      same request picks up exactly where the failure stopped.
 *    · REPORTED — the caller no longer gets a bare 400 carrying updateTask's
 *      error. It gets how many moved, where, that the column was NOT changed,
 *      and that a retry continues. That was the actual missing piece: the board
 *      state was already fine, the record of it was not. */
async function drainSignoffColumn(
  boardId: string,
  key: string,
  label: string,
  rowId: string,
  from: string,
  to: string,
  actor: string,
): Promise<void> {
  if (from === to) return
  const what = SIGNOFF_CATEGORIES[from]
  if (!what) return
  const sql = await db()
  const held = (await sql`
    select id from tasks where board_id = ${boardId} and status = ${key} order by created_at asc
  `) as unknown as Array<{ id: string }>
  const n = held.length
  if (!n) return
  const one = n === 1
  const tickets = `${n} ticket${one ? '' : 's'}`
  const siblings = (await sql`
    select key, label from board_statuses
    where board_id = ${boardId} and category = ${from} and id <> ${rowId}
    order by position, created_at
  `) as unknown as Array<{ key: string; label: string }>
  if (siblings.length === 0) {
    throw new Error(
      `"${label}" still holds ${tickets} ${what}, and it is this board's only ${from} column — there is nowhere of the same kind to move ${one ? 'it' : 'them'}, so changing the category here would release ${one ? 'it' : 'them'} with nothing on the record. Move ${one ? 'that ticket' : 'those tickets'} where you want ${one ? 'it' : 'them'} first, then change the category.`,
    )
  }
  if (siblings.length > 1) {
    const cols = siblings.map((s) => `"${s.label}"`).join(', ')
    throw new Error(
      `"${label}" still holds ${tickets} ${what}, and this board has ${siblings.length} other ${from} columns (${cols}) — which one ${one ? 'it belongs' : 'they belong'} in is your call, not ours. Move ${one ? 'that ticket' : 'those tickets'} first, then change the category.`,
    )
  }
  const dest = siblings[0]!
  // Dynamic import: server/tasks.ts imports this module (see deleteStatus).
  const { updateTask } = await import('./tasks')
  // BEFORE the category changes, for the same reason deleteStatus moves before
  // it drops the row: `updateTask` reads the board's columns as they are.
  let moved = 0
  for (const t of held) {
    try {
      await updateTask(
        t.id,
        { status: dest.key as TaskStatus, statusNote: `the "${label}" column was recategorised as "${to}"` },
        { kind: 'human', id: actor },
      )
    } catch (e) {
      const sofar = moved
        ? `${moved} of ${tickets} had already moved to "${dest.label}", where ${moved === 1 ? 'it is' : 'they are'} still ${what}`
        : `no ticket moved`
      throw new Error(
        `"${label}" was not recategorised: moving its tickets into "${dest.label}" stopped on one of them — ${(e as Error).message}. ${sofar}, and "${label}" still has its ${from} category, so nothing was released. Make the same change again once that is fixed: the tickets already moved are out of this column, so it carries on from where it stopped.`,
      )
    }
    moved++
  }
}

/** `review` and `agentStart` are opposite ends of the same handover: a review
 *  column is the queue a PERSON signs work off from, an agent-start column is
 *  the queue agents pick work UP from. Flagged both, an agent's hand-off drops
 *  straight back into its own pickup queue. The API route validates the
 *  EFFECTIVE post-patch column before it gets here; this is the invariant
 *  itself, so any future caller inherits it too. */
export const REVIEW_AGENT_START_CONFLICT =
  'a review column is where a person signs work off — it cannot also be an agent-start column agents pick work up from. Clear "agent start" on this column in the same change to make it a review column.'

/** The sibling rule, and it was missing: `agent_start` on a DONE column turns
 *  CLOSING a ticket into a dispatch. One legal
 *  `PUT {statusKey:'done', agentStart:true}` was accepted; a human then moving an
 *  agent-assigned ticket to 'done' hit updateTask's re-dispatch branch, which
 *  wrote a 'dispatch' activity row and started a live work session ON THE CLOSED
 *  TICKET. Terminal and pickup are opposite ends of the same handover, exactly
 *  like review and pickup. */
export const DONE_AGENT_START_CONFLICT =
  'a done column is where work is closed — it cannot also be an agent-start column agents pick work up from, or closing a ticket would dispatch fresh work on it. Clear "agent start" on this column in the same change to make it a done column.'

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
  // Same treatment, and it was missing: an owner may perfectly reasonably want a
  // column called "Cancelled", and the slug of that is the key that means
  // TERMINAL EVERYWHERE. `terminal()` then answers true for it whatever category
  // the owner chose, so the column drops out of `placeable` and out of every
  // destination a resolver can hand a ticket to — a column that silently cannot
  // receive work, on a board that looks fine. The label the owner typed is kept;
  // only the internal key moves out of the way, exactly as for `blocked`.
  // (Existing rows keep their key — `statusDiagnostics` reports those.)
  if (OFF_BOARD_STATUSES.includes(key)) key = `${key}_2`
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
 *  the virtual set into rows on first touch, then the key resolves.
 *
 *  `actor` is required for the same reason `deleteStatus` requires one:
 *  recategorising a populated sign-off column MOVES TICKETS, and a ticket move
 *  belongs to the person who caused it. Optional would mean a caller could
 *  forget, and the moves would land on the record unattributed. */
export async function updateStatus(
  boardId: string,
  key: string,
  patch: { label?: string; color?: string; category?: StatusCategory; agentStart?: boolean; position?: number },
  actor: string,
): Promise<void> {
  if (key === 'blocked') throw new Error('Blocked is a system status')
  await materialize(boardId)
  const sql = await db()
  const [cur] = (await sql`
    select id, label, category, agent_start as "agentStart" from board_statuses where key = ${key} and board_id = ${boardId}
  `) as unknown as Array<{ id: string; label: string; category: string; agentStart: boolean }>
  // A key that names nothing used to no-op and report success — a silent
  // failure, and one that hides a typo'd guard rather than surfacing it.
  if (!cur) throw new Error(`"${key}" is not a status on this board`)
  // The effective post-patch column, so neither flag can arrive alone and slip
  // the pair past the check. FIRST, before anything below writes: the drain
  // moves tickets, and a write that is going to be refused must not move any.
  // `PUT {review column, category:'done', agentStart:true}` is exactly that
  // write — legal-looking until the pair is checked, and its drain would have
  // emptied the column on the way to a 400.
  const conflict = agentStartConflict((patch.category ?? cur.category) as StatusCategory, patch.agentStart ?? cur.agentStart)
  if (conflict) throw new Error(conflict)
  // Recategorising is the same loss as deleting: symmetric with deleteStatus.
  if (patch.category !== undefined && patch.category !== cur.category) {
    await assertNotLastOfCategory(boardId, cur.id, cur.category)
    // …and it is the same ticket move as emptying the column by hand, so it is
    // made, not refused — through updateTask, exactly as deleteStatus does.
    await drainSignoffColumn(boardId, key, cur.label, cur.id, cur.category, patch.category, actor)
  }
  // ONE statement, not five. `category` and `agent_start` are read TOGETHER by
  // `agentStartConflict`, so five separate writes meant a failure between them
  // could leave the pair the checked patch was validated against split across
  // the column — a review column still flagged agent-start, refused on every
  // edit but sitting there. Addressed by id: the row was resolved above.
  const set: Record<string, unknown> = {}
  if (patch.label !== undefined) set.label = patch.label.trim()
  if (patch.color !== undefined) set.color = patch.color
  if (patch.category !== undefined) set.category = patch.category
  if (patch.agentStart !== undefined) set.agent_start = patch.agentStart
  if (patch.position !== undefined) set.position = patch.position
  if (Object.keys(set).length) await sql`update board_statuses set ${sql(set)} where id = ${cur.id}`
  publishBoard(boardId, { type: 'board' })
}

/** Replace the whole order (array of status KEYS in the new order).
 *
 *  THE ORDER IS A PERMUTATION, AND IT USED NOT TO BE CHECKED. `position = the
 *  array index` was written for whatever keys arrived, so an order that named a
 *  key the board does not have, or left one of its columns out, was accepted
 *  with a 200 and quietly produced an order nobody asked for:
 *  `{order:['inbox','ghost','done']}` had 'ghost' consume index 1 (matching no
 *  row), 'done' take index 2 — colliding with `in_progress`, still at 2 — and
 *  the board's real order then fell to `listStatuses`'s `created_at` tiebreak.
 *  Silent, and on the one operation whose whole job is to say where columns go.
 *
 *  So the set is validated as a permutation of this board's columns and the
 *  refusal names every part of the mismatch at once, rather than failing on the
 *  first and making the caller discover the rest one request at a time. The
 *  system Blocked column is not a row and is never listed here — `listStatuses`
 *  places it — so naming it is an unknown key like any other, which is exactly
 *  what the sentence says.
 *
 *  The writes ARE a transaction, unlike the drain: this touches nothing but
 *  `board_statuses.position` — no activity rows, no notifications, no dispatch —
 *  so there is nothing outside the database to leave half-done, and an
 *  interrupted reorder is the one failure that genuinely can be undone. */
export async function reorderStatuses(boardId: string, keys: string[]): Promise<void> {
  await materialize(boardId)
  const sql = await db()
  const rows = (await sql`select key from board_statuses where board_id = ${boardId}`) as unknown as Array<{ key: string }>
  const have = new Set(rows.map((r) => r.key))
  const seen = new Set<string>()
  const unknown: string[] = []
  const twice: string[] = []
  for (const k of keys) {
    if (!have.has(k)) unknown.push(k)
    else if (seen.has(k)) twice.push(k)
    seen.add(k)
  }
  const missing = [...have].filter((k) => !seen.has(k))
  if (unknown.length || twice.length || missing.length) {
    const q = (ks: string[]) => [...new Set(ks)].map((k) => `“${k}”`).join(', ')
    const parts: string[] = []
    if (unknown.length) parts.push(`${q(unknown)} ${unknown.length === 1 ? 'is not a column' : 'are not columns'} on this board`)
    if (twice.length) parts.push(`${q(twice)} ${twice.length === 1 ? 'is listed' : 'are listed'} more than once`)
    if (missing.length) parts.push(`${q(missing)} ${missing.length === 1 ? 'is' : 'are'} missing from the order`)
    throw new Error(`a new column order has to list every column on this board exactly once — ${parts.join('; ')}. Nothing was reordered.`)
  }
  await sql.begin(async (tx) => {
    for (let i = 0; i < keys.length; i++) {
      await tx`update board_statuses set position = ${i} where key = ${keys[i]!} and board_id = ${boardId}`
    }
  })
  publishBoard(boardId, { type: 'board' })
}

/** Delete a status; its tickets move to `reassignTo` (a status key). The last
 *  intake and the last terminal column are protected on every board, and the
 *  last review catch on every board that permits agents — statusMeta resolves
 *  each of those to a real key for whoever needs it. Same rule as updateStatus
 *  (see REQUIRED_CATEGORIES / AGENT_REQUIRED_CATEGORIES).
 *
 *  THE REASSIGNMENT IS AN ORDINARY STATUS MOVE, and `actor` is why this takes
 *  one. `assertSignoffColumnEmpty` REFUSES to recategorise a review column that
 *  still holds tickets, in as many words, because that "would release them back
 *  to the agents with nothing on the record saying they left review" — and its
 *  refusal recommends deleting the column instead. So the recommended path did
 *  the exact thing the refused path was blocked for, and rather more: a bulk
 *  `update tasks set status = …` writes no activity row, never stamps
 *  `completed_at` when the tickets land in a done column, tells no watcher, and
 *  never reaches the push side when they land in a pickup queue. Four things
 *  `updateTask` does, that a second writer of `tasks.status` has to remember.
 *  Adding the missing activity row would have made this the second writer that
 *  remembers ONE of the four — the shape every one of these rounds has been
 *  unpicking. So there is no second writer: each ticket moves through
 *  `updateTask`, as the person who deleted the column, and inherits all four.
 *  N updates instead of one statement is the price, and deleting a populated
 *  column is a rare admin action; being wrong four ways was the alternative. */
export async function deleteStatus(boardId: string, key: string, reassignTo: string, actor: string): Promise<void> {
  if (key === 'blocked') throw new Error('Blocked is a system status')
  await materialize(boardId)
  const sql = await db()
  const [victim] = (await sql`select id, key, label, category from board_statuses where key = ${key} and board_id = ${boardId}`) as unknown as Array<{
    id: string
    key: string
    label: string
    category: string
  }>
  // Same as updateStatus, and it was the asymmetric half: a key that names
  // nothing used to return 200 {ok:true} here, so DELETE of a typo'd or already-
  // deleted column reported that a column had been removed and its tickets
  // reassigned when neither had happened. Phase 0 called that shape "a silent
  // no-op" when it fixed it on updateStatus; the two are the same operation from
  // opposite ends and now say the same sentence.
  if (!victim) throw new Error(`"${key}" is not a status on this board`)
  await assertNotLastOfCategory(boardId, victim.id, victim.category)
  const meta = await statusMeta(boardId)
  if (!meta.keys.includes(reassignTo) || reassignTo === victim.key) throw new Error('pick a surviving status for its tickets')
  const doomed = (await sql`
    select id from tasks where board_id = ${boardId} and status = ${victim.key} order by created_at asc
  `) as unknown as Array<{ id: string }>
  // Dynamic import: server/tasks.ts imports this module, so a static one would
  // close the cycle.
  const { updateTask } = await import('./tasks')
  // BEFORE the column is dropped: `updateTask` refuses a status the board does
  // not have, and while this move is legal the one being vacated must still
  // resolve for the ticket it is reading.
  for (const t of doomed) {
    await updateTask(t.id, { status: reassignTo as TaskStatus, statusNote: `the "${victim.label}" column was deleted` }, { kind: 'human', id: actor })
  }
  await sql`delete from board_statuses where id = ${victim.id}`
  publishBoard(boardId, { type: 'board' })
}

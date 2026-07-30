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

/** Workflow metadata the task engine needs, resolved per board. */
export async function statusMeta(boardId: string): Promise<{
  keys: string[]
  agentStartKeys: string[]
  reviewKey: string
  reviewKeys: string[]
  doneKeys: string[]
  defaultKey: string
  assignedKey: string
}> {
  const list = await listStatuses(boardId)
  const agentStart = list.filter((s) => s.agentStart).map((s) => s.key)
  const reviews = list.filter((s) => s.category === 'review').map((s) => s.key)
  const done = list.filter((s) => s.category === 'done').map((s) => s.key)
  const open = list.find((s) => s.category === 'open')?.key ?? list[0]!.key
  return {
    keys: list.map((s) => s.key),
    agentStartKeys: agentStart,
    reviewKey: reviews[0] ?? 'quality_review',
    reviewKeys: reviews.length ? reviews : ['quality_review'],
    doneKeys: done.length ? done : ['done'],
    defaultKey: open,
    assignedKey: agentStart[0] ?? open,
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

export async function createStatus(
  boardId: string,
  input: { label: string; color?: string; category?: StatusCategory; agentStart?: boolean },
): Promise<BoardStatus> {
  await materialize(boardId)
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
 *  open-category status and the last review-category status are protected —
 *  the workflow needs an intake and a review catch. */
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
  if (victim.category === 'open' || victim.category === 'review') {
    const others = await sql`
      select 1 from board_statuses where board_id = ${boardId} and category = ${victim.category} and id <> ${victim.id}
    `
    if (!others.length) throw new Error(`the workflow needs at least one ${victim.category === 'open' ? 'intake' : 'review'} status`)
  }
  const meta = await statusMeta(boardId)
  if (!meta.keys.includes(reassignTo) || reassignTo === victim.key) throw new Error('pick a surviving status for its tickets')
  await sql`update tasks set status = ${reassignTo}, updated_at = now() where board_id = ${boardId} and status = ${victim.key}`
  await sql`delete from board_statuses where id = ${victim.id}`
  publishBoard(boardId, { type: 'board' })
}

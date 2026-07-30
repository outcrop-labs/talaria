// Board labels — first-class, colored, scoped to a board. Task.tags stays a
// plain string array of label NAMES (agents and old data keep working);
// ensureLabels auto-registers any name that reaches a ticket, so the registry
// is always complete and always the place to manage from. Renames cascade
// into every ticket's tags; deletes strip the label off tickets.
import { db } from './db/pg'
import { publishBoard } from './realtime'

export const LABEL_COLOR_KEYS = ['slate', 'bronze', 'green', 'amber', 'red', 'blue', 'purple', 'teal'] as const
export type LabelColor = (typeof LABEL_COLOR_KEYS)[number]

export interface BoardLabel {
  id: string
  boardId: string
  name: string
  color: LabelColor
  position: number
}

const ROW = `id, board_id as "boardId", name, color, position`

export async function listLabels(boardId: string): Promise<BoardLabel[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from board_labels where board_id = $1 order by position, lower(name)`, [
    boardId,
  ])) as unknown as BoardLabel[]
}

export async function createLabel(boardId: string, name: string, color?: string): Promise<BoardLabel> {
  const sql = await db()
  const n = name.trim()
  if (!n) throw new Error('label name required')
  const c = LABEL_COLOR_KEYS.includes(color as LabelColor) ? color : 'slate'
  const rows = (await sql`
    insert into board_labels (board_id, name, color) values (${boardId}, ${n}, ${c!})
    on conflict (board_id, name) do update set color = excluded.color
    returning ${sql.unsafe(ROW)}
  `) as unknown as BoardLabel[]
  return rows[0]!
}

/** Register any names that reached a ticket but aren't labels yet (agents,
 *  MCP, old callers). Keeps free-string writes working AND manageable. */
export async function ensureLabels(boardId: string, names: string[]): Promise<void> {
  const clean = [...new Set(names.map((n) => n.trim()).filter(Boolean))]
  if (clean.length === 0) return
  const sql = await db()
  for (const n of clean) {
    await sql`insert into board_labels (board_id, name) values (${boardId}, ${n}) on conflict do nothing`
  }
}

export async function updateLabel(
  boardId: string,
  labelId: string,
  patch: { name?: string; color?: string },
): Promise<void> {
  const sql = await db()
  const [cur] = (await sql`select name from board_labels where id = ${labelId} and board_id = ${boardId}`) as unknown as Array<{
    name: string
  }>
  if (!cur) throw new Error('no such label')
  if (patch.color !== undefined) {
    if (!LABEL_COLOR_KEYS.includes(patch.color as LabelColor)) throw new Error('unknown color')
    await sql`update board_labels set color = ${patch.color} where id = ${labelId}`
  }
  const next = patch.name?.trim()
  if (next && next !== cur.name) {
    await sql`update board_labels set name = ${next} where id = ${labelId}`
    // Rename cascades into every ticket carrying the old name.
    await sql`
      update tasks set tags = to_jsonb(array(
        select case when e = ${cur.name} then ${next} else e end
        from jsonb_array_elements_text(tags) as e
      )), updated_at = now()
      where board_id = ${boardId} and tags ? ${cur.name}
    `
  }
  publishBoard(boardId, { type: 'board' })
}

export async function deleteLabel(boardId: string, labelId: string): Promise<void> {
  const sql = await db()
  const [cur] = (await sql`select name from board_labels where id = ${labelId} and board_id = ${boardId}`) as unknown as Array<{
    name: string
  }>
  if (!cur) return
  await sql`delete from board_labels where id = ${labelId}`
  // Strip it off every ticket.
  await sql`
    update tasks set tags = to_jsonb(array(
      select e from jsonb_array_elements_text(tags) as e where e <> ${cur.name}
    )), updated_at = now()
    where board_id = ${boardId} and tags ? ${cur.name}
  `
  publishBoard(boardId, { type: 'board' })
}

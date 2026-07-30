// Capability gaps — the honesty loop's memory. The contract with agents:
// competence first, and when work genuinely can't be done properly (missing
// tools/access, org-specific process the agent would be guessing at), report
// the gap instead of improvising. The contract with humans: no nagging —
// one row per work-shape ever, repeats bump seen_count (frequency is ranking
// signal), and delivery is the Studio's Suggested queue, not inbox pings.
import { db } from './db/pg'

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

export async function reportGap(input: {
  agentModel: string
  kind: string
  missing: string
  needs?: string
  boardId?: string | null
  taskId?: string | null
}): Promise<{ id: string; seenCount: number; first: boolean }> {
  const sql = await db()
  const sig = signatureOf(input.boardId ?? null, input.kind)
  const rows = (await sql`
    insert into capability_gaps (signature, kind, board_id, agent_model, missing, needs, example_task_id)
    values (${sig}, ${slug(input.kind)}, ${input.boardId ?? null}, ${input.agentModel},
            ${input.missing.slice(0, 300)}, ${input.needs?.slice(0, 5000) ?? ''}, ${input.taskId ?? null})
    on conflict (signature) do update set
      seen_count = capability_gaps.seen_count + 1,
      last_seen = now(),
      -- a dismissed shape that keeps recurring reopens; resolved stays resolved
      status = case when capability_gaps.status = 'dismissed' then 'open' else capability_gaps.status end,
      example_task_id = coalesce(capability_gaps.example_task_id, excluded.example_task_id)
    returning id, seen_count as "seenCount", (seen_count = 1) as first
  `) as unknown as Array<{ id: string; seenCount: number; first: boolean }>
  return rows[0]!
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

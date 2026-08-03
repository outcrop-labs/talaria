// Boards — user-owned kanban boards, shareable across the team. Membership is
// the share mechanism (role: owner | editor | viewer).
import { db } from './db/pg'
import { isElevatedAssistant } from './users'
import { subjectModel, type AgentSubject } from './agent-auth'

export type BoardRole = 'owner' | 'editor' | 'viewer'

export interface Board {
  id: string
  name: string
  ownerId: string
  teamId: string | null
  teamName: string | null
  role: BoardRole // the requesting user's role
  createdAt: string
  updatedAt: string
  archivedAt: string | null
}

const RANK: Record<BoardRole, number> = { owner: 3, editor: 2, viewer: 1 }

export interface BoardMember {
  userId: string
  email: string | null
  name: string | null
  role: BoardRole
}

/** Boards the user can see — explicitly shared OR via a team they belong to.
 *  Archived boards are hidden unless `archived` is requested. */
export async function listBoards(userId: string, archived = false): Promise<Board[]> {
  const sql = await db()
  const rows = await sql`
    select b.id, b.name, b.owner_id as "ownerId", b.team_id as "teamId", t.name as "teamName",
           coalesce(m.role, case when tm.role = 'owner' then 'owner' when tm.role is not null then 'editor' end) as role,
           b.judge_mode as "judgeMode",
           b.created_at as "createdAt", b.updated_at as "updatedAt", b.archived_at as "archivedAt"
    from boards b
    left join board_members m on m.board_id = b.id and m.user_id = ${userId}
    left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    left join teams t on t.id = b.team_id
    where (m.user_id is not null or tm.user_id is not null)
      and b.archived_at is ${archived ? sql`not null` : sql`null`}
    order by b.updated_at desc
  `
  return rows as unknown as Board[]
}

/** The user's effective role on a board — max of explicit share + team access. */
export async function boardRole(userId: string, boardId: string): Promise<BoardRole | null> {
  const sql = await db()
  const rows = await sql`
    select role from board_members where board_id = ${boardId} and user_id = ${userId}
    union all
    select case when tm.role = 'owner' then 'owner' else 'editor' end as role
    from boards b join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    where b.id = ${boardId}
  `
  const roles = (rows as unknown as Array<{ role: BoardRole }>).map((r) => r.role)
  if (roles.length === 0) return null
  return roles.sort((a, b) => RANK[b] - RANK[a])[0]!
}

export const canEdit = (role: BoardRole | null) => role === 'owner' || role === 'editor'

/** Existence + name + ARCHIVAL STATE, in one place.
 *
 *  `archivedAt` is the load-bearing field and the reason this lives here rather
 *  than in each caller: archival is one fact about a board, and it was being
 *  read (or forgotten) independently by `boardAllowsAgent`, by the agent-write
 *  predicate in server/tasks.ts, and by the heartbeat query — which is how an
 *  agent ended up unable to see an archived board, unable to patch its tickets,
 *  and yet able to CREATE one on it. `label` is for diagnostics and falls back
 *  to the id. */
export async function boardInfo(boardId: string): Promise<{ label: string; archivedAt: string | null; exists: boolean }> {
  const sql = await db()
  const rows = (await sql`select name, archived_at as "archivedAt" from boards where id = ${boardId}`) as unknown as Array<{
    name: string | null
    archivedAt: string | Date | null
  }>
  const row = rows[0]
  return {
    label: row?.name ? `"${row.name}"` : boardId,
    archivedAt: row?.archivedAt ? new Date(row.archivedAt).toISOString() : null,
    exists: !!row,
  }
}

/** A short uppercase ticket prefix from the board name (e.g. "Sprint Board" → "SB"). */
function ticketPrefix(name: string): string {
  const initials = name
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((w) => w[0]!)
    .join('')
    .toUpperCase()
    .slice(0, 4)
  return initials.length >= 2 ? initials : (name.replace(/[^a-zA-Z0-9]/g, '').toUpperCase().slice(0, 4) || 'TASK')
}

/** Create a board (personal, or under a team) and make the creator its owner. */
export async function createBoard(userId: string, name: string, teamId?: string | null): Promise<Board> {
  const sql = await db()
  const board = await sql.begin(async (tx) => {
    const rows = await tx`
      insert into boards (name, owner_id, ticket_prefix, team_id)
      values (${name}, ${userId}, ${ticketPrefix(name)}, ${teamId ?? null})
      returning id, name, owner_id as "ownerId", team_id as "teamId", created_at as "createdAt", updated_at as "updatedAt"
    `
    const b = rows[0] as Omit<Board, 'role' | 'teamName'>
    await tx`insert into board_members (board_id, user_id, role) values (${b.id}, ${userId}, 'owner')`
    return b
  })
  return { ...board, teamName: null, role: 'owner', archivedAt: null }
}

export async function setBoardJudgeMode(boardId: string, mode: 'inherit' | 'off' | 'advisory' | 'enforcing'): Promise<void> {
  const sql = await db()
  await sql`update boards set judge_mode = ${mode}, updated_at = now() where id = ${boardId}`
}

export async function renameBoard(boardId: string, name: string): Promise<void> {
  const sql = await db()
  await sql`update boards set name = ${name}, updated_at = now() where id = ${boardId}`
}

/** Move a board between teams (null → personal). The team must exist. */
export async function setBoardTeam(boardId: string, teamId: string | null): Promise<void> {
  const sql = await db()
  if (teamId) {
    const rows = await sql`select 1 from teams where id = ${teamId}`
    if (rows.length === 0) throw new Error('unknown team')
  }
  await sql`update boards set team_id = ${teamId}, updated_at = now() where id = ${boardId}`
}

export async function archiveBoard(boardId: string, archived: boolean): Promise<void> {
  const sql = await db()
  await sql`update boards set archived_at = ${archived ? sql`now()` : null}, updated_at = now() where id = ${boardId}`
}

export async function deleteBoard(boardId: string): Promise<void> {
  const sql = await db()
  await sql`delete from boards where id = ${boardId}`
}

export async function listMembers(boardId: string): Promise<BoardMember[]> {
  const sql = await db()
  const rows = await sql`
    select m.user_id as "userId", u.email, u.name, m.role
    from board_members m join users u on u.id = m.user_id
    where m.board_id = ${boardId}
    order by (m.role = 'owner') desc, u.email asc
  `
  return rows as unknown as BoardMember[]
}

/** Share a board with a teammate by email (they must have signed in before). */
export async function shareBoard(
  boardId: string,
  email: string,
  role: BoardRole,
): Promise<{ ok: true } | { ok: false; error: string }> {
  const sql = await db()
  const users = await sql`select id from users where lower(email) = ${email.trim().toLowerCase()}`
  if (users.length === 0) return { ok: false, error: 'No user with that email has signed in yet' }
  const userId = (users[0] as { id: string }).id
  await sql`
    insert into board_members (board_id, user_id, role) values (${boardId}, ${userId}, ${role})
    on conflict (board_id, user_id) do update set role = excluded.role
  `
  return { ok: true }
}

export async function unshareBoard(boardId: string, userId: string): Promise<void> {
  const sql = await db()
  // Never remove the owner via unshare.
  await sql`delete from board_members where board_id = ${boardId} and user_id = ${userId} and role <> 'owner'`
}

// ── Board-scoped agents ──────────────────────────────────────────────────────
export interface BoardAgentConfig {
  allowAll: boolean
  models: string[]
}

/** A board's agent policy: allow-all flag + the explicit allow-list. */
export async function getBoardAgentConfig(boardId: string): Promise<BoardAgentConfig> {
  const sql = await db()
  const b = await sql`select allow_all_agents as "allowAll" from boards where id = ${boardId}`
  const rows = await sql`select agent_model from board_agents where board_id = ${boardId} order by agent_model`
  return {
    allowAll: !!(b[0] as { allowAll?: boolean } | undefined)?.allowAll,
    models: (rows as unknown as Array<{ agent_model: string }>).map((r) => r.agent_model),
  }
}

export async function setBoardAgentConfig(boardId: string, allowAll: boolean, models: string[]): Promise<void> {
  const sql = await db()
  await sql.begin(async (tx) => {
    await tx`update boards set allow_all_agents = ${allowAll} where id = ${boardId}`
    await tx`delete from board_agents where board_id = ${boardId}`
    if (!allowAll) {
      for (const m of models) {
        await tx`insert into board_agents (board_id, agent_model) values (${boardId}, ${m}) on conflict do nothing`
      }
    }
  })
}

/** Whether an agent may be assigned on a board. Restrictive by default — a board
 *  allows an agent only if allow-all is on OR the agent is explicitly listed.
 *  An admin-elevated personal assistant passes everywhere (org-wide access) —
 *  but only when the CALLER proved it is that assistant: pass the AgentCaller,
 *  not its model, wherever one is in hand.
 *
 *  A bare model string remains accepted for the one shape that has NO caller:
 *  policy questions about a third party (invalidAssignee below, board config).
 *  `subjectProven` reads a bare string as proven, so passing `caller.model`
 *  where `caller` is in hand silently discards the legacy flag — every route
 *  under this repo's ownership now passes the caller.
 *
 *  ARCHIVAL IS THE THIRD END OF THE SAME RULE. `listBoardsForAgent` and
 *  `listAllBoards` already hide archived boards, and `closedToAgents` refuses
 *  writes to a ticket on one — but this predicate, the gate every agent-facing
 *  board route stands behind, ignored `archived_at` entirely. So an agent could
 *  not SEE an archived board and could not PATCH its tickets, yet
 *  `POST /api/boards/:id/tasks` handed it a fresh ticket ref on one. An
 *  archived board is out of service, for every agent, at every door — including
 *  the elevated assistant, whose bypass is a policy exemption, not a licence to
 *  work retired boards. */
export async function boardAllowsAgent(boardId: string, agent: AgentSubject): Promise<boolean> {
  const board = await boardInfo(boardId)
  if (!board.exists || board.archivedAt) return false
  const cfg = await getBoardAgentConfig(boardId)
  if (cfg.allowAll || cfg.models.includes(subjectModel(agent))) return true
  return isElevatedAssistant(agent)
}

/** Validate a mixed assignee list: `user:<uuid>` entries must be board
 *  members; bare strings are agents and must pass the board's agent policy.
 *  Returns the first human-readable problem, or null when all pass. */
export async function invalidAssignee(boardId: string, assignees: string[]): Promise<string | null> {
  for (const a of assignees) {
    if (a.startsWith('user:')) {
      if (!(await boardRole(a.slice(5), boardId))) return 'assignees must be members of this board'
    } else if (!(await boardAllowsAgent(boardId, a))) {
      return `agent "${a}" is not allowed on this board`
    }
  }
  return null
}

/** Every live board, org-wide — the elevated-assistant listing. No role. */
export async function listAllBoards(): Promise<Array<Omit<Board, 'role'>>> {
  const sql = await db()
  const rows = await sql`
    select b.id, b.name, b.owner_id as "ownerId", b.team_id as "teamId", t.name as "teamName",
           b.created_at as "createdAt", b.updated_at as "updatedAt", b.archived_at as "archivedAt"
    from boards b left join teams t on t.id = b.team_id
    where b.archived_at is null order by b.updated_at desc
  `
  return rows as unknown as Array<Omit<Board, 'role'>>
}

/** Boards an agent may work on (allow-all boards + boards listing it). The
 *  agent-facing shape carries no membership role. */
export async function listBoardsForAgent(model: string): Promise<Array<Omit<Board, 'role'>>> {
  const sql = await db()
  const rows = await sql`
    select distinct b.id, b.name, b.owner_id as "ownerId", b.team_id as "teamId", t.name as "teamName",
           b.created_at as "createdAt", b.updated_at as "updatedAt", b.archived_at as "archivedAt"
    from boards b
    left join board_agents a on a.board_id = b.id and a.agent_model = ${model}
    left join teams t on t.id = b.team_id
    where (b.allow_all_agents or a.agent_model is not null) and b.archived_at is null
    order by b.updated_at desc
  `
  return rows as unknown as Array<Omit<Board, 'role'>>
}

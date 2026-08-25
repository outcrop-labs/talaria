// The Home/Today summary — the seamless landing. In Talaria's guardrail model
// a person's job is to triage, review, and unblock the agents' work, so Home
// surfaces exactly those queues (scoped to boards the user can see) plus unread
// mentions. One round-trip, cheap aggregates.
import { db } from './db/pg'
import { activityFeed, type ActivityEvent } from './activity-feed'
import { computeAlerts } from './alerts'
import { orgProfile } from './org'
import { costOverview } from './usage'

export interface WorkItem {
  id: string
  boardId: string
  board: string
  ticketRef: string | null
  title: string
  status: string
  updatedAt: string
}

export interface OrgGlance {
  /** The business name (Admin → Organization), for the rail's title. */
  name: string
  /** A compact recent-activity pulse across the workspace. */
  activity: ActivityEvent[]
  /** Admin-only: live alert count (null for members). */
  alerts: number | null
  /** Admin-only: today's metered spend (null for members). */
  costToday: { tokens: number; usd: number } | null
}

/** The three human queues, scoped to what this user can see. */
export interface HomeQueues {
  triage: { count: number; items: WorkItem[] } // inbox — needs a human to assign
  review: { count: number; items: WorkItem[] } // quality_review — needs sign-off
  blocked: { count: number; items: WorkItem[] } // blocked — needs unblocking
}

export interface HomeSummary {
  org: OrgGlance
  queues: HomeQueues
  unread: number
  boards: number
  fleet: { online: number; total: number; down: string[] }
}

// Full lists — the Boards console shows everything per queue (capped sanely).
const WINDOW = 100

/** The queue pass on its own, without the org/fleet glance around it.
 *
 *  Split out for the daily digest (server/digest.ts), which needs exactly these
 *  three numbers for every user in the workspace and none of the glance. Calling
 *  `homeSummary` per user would have run `containerStatus` — a Docker round trip
 *  — once per recipient, and an email that says "2 tickets in QA" must count
 *  them the SAME WAY the screen does, so copying the query into the digest was
 *  never an option. One definition, two callers. */
export async function homeQueues(userId: string): Promise<HomeQueues> {
  const sql = await db()

  // One pass over the user's visible, active tickets in the three human queues.
  const rows = (await sql`
    select t.id, t.board_id as "boardId", b.name as board,
           case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as "ticketRef",
           t.title, t.status, t.updated_at as "updatedAt",
           case
             when t.status = 'blocked' then 'blocked'
             when t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'review')
               or (not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) and t.status = 'quality_review')
               then 'review'
             else 'triage'
           end as "queue"
    from tasks t
    join boards b on b.id = t.board_id
    left join board_members m on m.board_id = b.id and m.user_id = ${userId}
    left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    where (m.user_id is not null or tm.user_id is not null)
      and b.archived_at is null and t.archived_at is null
      and (
        t.status = 'blocked'
        or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'review')
        or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start)
        or (
          not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
          and t.status in ('inbox', 'quality_review')
        )
      )
    order by t.updated_at desc
  `) as unknown as WorkItem[]

  const bucket = (queue: string): { count: number; items: WorkItem[] } => {
    const items = (rows as Array<WorkItem & { queue: string }>).filter((r) => r.queue === queue)
    return { count: items.length, items: items.slice(0, WINDOW) }
  }

  return { triage: bucket('triage'), review: bucket('review'), blocked: bucket('blocked') }
}

export async function homeSummary(userId: string, role: 'admin' | 'member' = 'member'): Promise<HomeSummary> {
  const sql = await db()
  const queues = await homeQueues(userId)

  const [{ unread }] = (await sql`
    select count(*)::int as unread from notifications where user_id = ${userId} and read_at is null
  `) as unknown as [{ unread: number }]

  const [{ boards }] = (await sql`
    select count(distinct b.id)::int as boards
    from boards b
    left join board_members m on m.board_id = b.id and m.user_id = ${userId}
    left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    where (m.user_id is not null or tm.user_id is not null) and b.archived_at is null
  `) as unknown as [{ boards: number }]

  // The org half of the glance: name, an activity pulse everyone sees, and
  // (admins) live alerts + today's spend. Failures degrade to quiet, never 500.
  // (Fleet health is NOT here: it left Home with the Fleet tab — Agents and
  // Observability own that question, and a docker status call per home load
  // was a tax every user paid for a glance only admins ever saw.)
  const isAdmin = role === 'admin'
  const [profile, activity, alertCount, cost] = await Promise.all([
    orgProfile().catch(() => ({ name: '', about: '' })),
    activityFeed(userId, [], 8, isAdmin).catch(() => []),
    isAdmin ? computeAlerts(userId).then((a) => a.length).catch(() => null) : Promise.resolve(null),
    isAdmin ? costOverview().catch(() => null) : Promise.resolve(null),
  ])

  return {
    org: {
      name: profile.name,
      activity,
      alerts: alertCount,
      costToday: cost
        ? { tokens: cost.totals.today.prompt + cost.totals.today.completion, usd: cost.totals.today.cost }
        : null,
    },
    queues,
    unread,
    boards,
  }
}

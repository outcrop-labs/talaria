// The Home/Today summary — the seamless landing. In Talaria's guardrail model
// a person's job is to triage, review, and unblock the agents' work, so Home
// surfaces exactly those queues (scoped to boards the user can see) plus unread
// mentions and a glance at fleet health. One round-trip, cheap aggregates.
import { db } from './db/pg'
import { activityFeed, type ActivityEvent } from './activity-feed'
import { computeAlerts } from './alerts'
import { containerStatus } from './fleet-docker'
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

export interface HomeSummary {
  org: OrgGlance
  queues: {
    triage: { count: number; items: WorkItem[] } // inbox — needs a human to assign
    review: { count: number; items: WorkItem[] } // quality_review — needs sign-off
    blocked: { count: number; items: WorkItem[] } // blocked — needs unblocking
  }
  unread: number
  boards: number
  fleet: { online: number; total: number; down: string[] }
}

const WINDOW = 5

export async function homeSummary(userId: string, role: 'admin' | 'member' = 'member'): Promise<HomeSummary> {
  const sql = await db()

  // One pass over the user's visible, active tickets in the three human queues.
  const rows = (await sql`
    select t.id, t.board_id as "boardId", b.name as board,
           case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as "ticketRef",
           t.title, t.status, t.updated_at as "updatedAt"
    from tasks t
    join boards b on b.id = t.board_id
    left join board_members m on m.board_id = b.id and m.user_id = ${userId}
    left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    where (m.user_id is not null or tm.user_id is not null)
      and b.archived_at is null and t.archived_at is null
      and t.status in ('inbox', 'quality_review', 'blocked')
    order by t.updated_at desc
  `) as unknown as WorkItem[]

  const bucket = (status: string): { count: number; items: WorkItem[] } => {
    const items = rows.filter((r) => r.status === status)
    return { count: items.length, items: items.slice(0, WINDOW) }
  }

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

  // Fleet health glance — container reality for enabled managed agents (the
  // same truth /alerts shows), so Home reflects what's actually running.
  const managed = (await sql`
    select department, display_name as "displayName" from agent_defs
    where managed and enabled order by slug
  `) as unknown as Array<{ department: string; displayName: string }>
  const states = managed.length ? await containerStatus(managed.map((m) => m.department)).catch(() => null) : []
  const down =
    states === null
      ? []
      : managed
          .filter((m) => states.find((s) => s.department === m.department)?.managed?.state !== 'running')
          .map((m) => m.displayName)

  // The org half of the glance: name, an activity pulse everyone sees, and
  // (admins) live alerts + today's spend. Failures degrade to quiet, never 500.
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
    queues: { triage: bucket('inbox'), review: bucket('quality_review'), blocked: bucket('blocked') },
    unread,
    boards,
    fleet: { online: managed.length - down.length, total: managed.length, down },
  }
}

// The fleet-wide activity feed: one merged stream of what's been happening
// across the workspace, scoped to the requesting user. Three sources:
//   • ticket events (task_activity)  — only boards the user can access
//   • channel messages               — only channels the user is a member of
//   • agent config changes (agent_versions) — visible to everyone (the fleet
//     is shared infrastructure; /agents itself isn't admin-gated)
// No new tables — this is a read model over what the app already records.
import { db } from './db/pg'

export type ActivityKind = 'ticket' | 'channel' | 'fleet' | 'audit'

export interface ActivityEvent {
  at: string
  kind: ActivityKind
  /** Who did it — email, agent model, or 'system'. */
  actor: string
  /** Where it happened — board name, #channel, or agent name. */
  context: string
  /** What happened, human-sized. */
  detail: string
  /** The event's own type within its source — 'dispatch', 'status', 'gap',
   *  an audit action like 'fleet.render' — for row labels and sub-filters. */
  type: string
  href: string
}

export async function activityFeed(
  userId: string,
  kinds: ActivityKind[],
  limit = 80,
  isAdmin = false,
): Promise<ActivityEvent[]> {
  const sql = await db()
  // Audit events are governance data — admins only; default set excludes them.
  const defaultKinds: ActivityKind[] = ['ticket', 'channel', 'fleet']
  const want = new Set(kinds.length ? kinds : defaultKinds)
  if (!isAdmin) want.delete('audit')
  const per = Math.min(limit, 80)

  const [tickets, channels, fleet, audit] = await Promise.all([
    want.has('ticket')
      ? sql`
          select a.created_at as at, a.actor, a.type, b.name as board, t.title,
                 a.description as detail, b.id as board_id, t.id as task_id
          from task_activity a
          join tasks t on t.id = a.task_id
          join boards b on b.id = t.board_id
          left join board_members m on m.board_id = b.id and m.user_id = ${userId}
          left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
          where m.user_id is not null or tm.user_id is not null
          order by a.created_at desc limit ${per}
        `
      : Promise.resolve([]),
    want.has('channel')
      ? sql`
          select m.created_at as at, m.author, m.author_type as "authorType",
                 c.name as channel, left(m.content, 160) as detail
          from channel_messages m
          join channels c on c.id = m.channel_id
          join channel_members cm on cm.channel_id = c.id and cm.user_id = ${userId}
          where m.status = 'complete' and m.content <> ''
          order by m.created_at desc limit ${per}
        `
      : Promise.resolve([]),
    want.has('fleet')
      ? sql`
          select v.created_at as at, coalesce(v.created_by, 'system') as actor,
                 d.display_name as agent, v.version, coalesce(v.note, '') as note
          from agent_versions v
          join agent_defs d on d.id = v.agent_id
          order by v.created_at desc limit ${per}
        `
      : Promise.resolve([]),
    want.has('audit')
      ? sql`
          select created_at as at, actor, action, target_type as "targetType",
                 target_label as "targetLabel", after
          from audit_log order by created_at desc limit ${per}
        `
      : Promise.resolve([]),
  ])

  const events: ActivityEvent[] = [
    ...(tickets as unknown as Array<Record<string, string>>).map((r) => ({
      at: r.at!,
      kind: 'ticket' as const,
      actor: r.actor!,
      context: r.board!,
      detail: `${r.title}: ${r.detail}`,
      type: r.type ?? 'activity',
      href: `/boards/${r.board_id}/${r.task_id}`,
    })),
    ...(channels as unknown as Array<Record<string, string>>).map((r) => ({
      at: r.at!,
      kind: 'channel' as const,
      actor: r.author!,
      context: `#${r.channel}`,
      detail: r.detail!,
      type: r.authorType === 'agent' ? 'agent' : 'message',
      href: '/channels',
    })),
    ...(fleet as unknown as Array<Record<string, string>>).map((r) => ({
      at: r.at!,
      kind: 'fleet' as const,
      actor: r.actor!,
      context: r.agent!,
      detail: `config v${r.version}${r.note ? `: ${r.note}` : ''}`,
      type: 'config',
      href: '/agents',
    })),
    ...(audit as unknown as Array<Record<string, unknown>>).map((r) => ({
      at: r.at as string,
      kind: 'audit' as const,
      actor: r.actor as string,
      context: String(r.action).split('.')[0] ?? 'audit',
      detail: `${(r.targetLabel as string) ?? (r.targetType as string)}${r.after ? ` → ${JSON.stringify(r.after)}` : ''}`.slice(0, 200),
      type: r.action as string,
      href: '/observability/audit',
    })),
  ]

  return events.sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime()).slice(0, limit)
}

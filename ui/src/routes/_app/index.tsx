import { createFileRoute, Link, useNavigate } from '@tanstack/react-router'
import { useQuery, useQueryClient, type UseQueryResult } from '@tanstack/react-query'
import { useState } from 'react'
import { Sparkles, CalendarDays, ChevronDown, ChevronRight, Plus, ExternalLink, Mail, Send, ShieldCheck, Check } from 'lucide-react'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { Panel } from '@/components/ui/panel'
import { alert } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Modal } from '@/components/ui/modal'
import { Tabs } from '@/components/ui/tabs'
import { Chip } from '@/components/ui/chip'
import { EmptyState } from '@/components/ui/empty-state'
import { listQuery, QueryError } from '@/components/ui/query-state'
import { getJson, getList, readJson } from '@/lib/fetch-json'
import { AssistantWizard } from '@/components/assistant/assistant-wizard'
import { NotificationsPanel } from '@/components/app/notifications-panel'
import { ActivityRow } from '@/components/app/activity-row'
import { relativeTime } from '@/lib/fleet'
import { cn } from '@/lib/cn'
import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu'
import { useAssistant } from '@/lib/assistant'
import { useSession } from '@/lib/session'
import { useChannels } from '@/lib/channels'
import { useConversations } from '@/lib/conversations'
import { useResearchRuns } from '@/lib/research'
import { useArtifacts } from '@/lib/artifacts'
import { useAgents } from '@/lib/agents'
import { parseAgentStream } from '@/lib/sse-parse'
import { Markdown } from '@/components/ui/markdown'

const HOME_TABS = ['inbox', 'boards', 'comms', 'plans', 'research', 'docs', 'fleet'] as const
type HomeTab = (typeof HOME_TABS)[number]

export const Route = createFileRoute('/_app/')({
  component: HomePage,
  // /?tab=boards deep-links a console tab.
  validateSearch: (search: Record<string, unknown>): { tab?: HomeTab } => {
    const t = HOME_TABS.find((v) => v === search.tab)
    return t ? { tab: t } : {}
  },
})

interface WorkItem {
  id: string
  boardId: string
  board: string
  ticketRef: string | null
  title: string
  status: string
  updatedAt: string
}
interface Queue {
  count: number
  items: WorkItem[]
}
interface OrgActivity {
  at: string
  kind: string
  actor: string
  context: string
  detail: string
  href: string
}

interface HomeSummary {
  org: {
    name: string
    activity: OrgActivity[]
    alerts: number | null
    costToday: { tokens: number; usd: number } | null
  }
  queues: { triage: Queue; review: Queue; blocked: Queue }
  unread: number
  boards: number
  fleet: { online: number; total: number; down: string[] }
}

const useHome = () =>
  useQuery({
    queryKey: ['home'],
    queryFn: (): Promise<HomeSummary> => getJson<HomeSummary>('/api/home'),
    refetchInterval: 30_000,
  })

// Time-aware and lightly varied: the pool is keyed by hour + day-of-year so
// the pick is stable across re-renders (no flicker) but changes through the
// day and from day to day.
const greeting = (name?: string | null) => {
  const who = name?.split(' ')[0] ?? name ?? 'there'
  const now = new Date()
  const h = now.getHours()
  const pools: Array<[boolean, string[]]> = [
    [h < 5, [`Up late, ${who}?`, `Quiet hours, ${who}`, `Night shift, ${who}?`]],
    [h < 12, [`Morning, ${who}`, `Good morning, ${who}`, `Fresh start, ${who}`]],
    [h < 17, [`Afternoon, ${who}`, `Good afternoon, ${who}`, `Back at it, ${who}`]],
    [h < 22, [`Evening, ${who}`, `Good evening, ${who}`, `Winding down, ${who}?`]],
    [true, [`Late one, ${who}?`, `Still here, ${who}?`]],
  ]
  const pool = pools.find(([match]) => match)![1]
  const dayOfYear = Math.floor((now.getTime() - new Date(now.getFullYear(), 0, 0).getTime()) / 86_400_000)
  return pool[(dayOfYear + h) % pool.length]!
}

// Home/Today — the seamless landing. Surfaces the human's real job in Talaria's
// guardrail model (triage · review · unblock), unread mentions, fleet health,
// and one-tap entries into the work surfaces.
function HomePage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const home = useHome()
  const { data } = home
  // Feeds the Comms badge count. Defaulted, a failed read renders "no unread" —
  // the one thing a badge exists to deny. Suppressing the badge is only half the
  // fix: a missing badge is indistinguishable from a genuine zero, and the lie
  // is told from EVERY tab, not just Comms, so CommsTab's own notice cannot
  // cover for it. The marker goes under the tab strip, where it is visible
  // wherever you are standing.
  const channelsList = listQuery(useChannels(), { title: 'Unread counts unavailable', variant: 'inline' })
  const channels = channelsList.rows
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const tab: HomeTab = search.tab ?? 'inbox'
  const setTab = (t: HomeTab) => void navigate({ search: t === 'inbox' ? {} : { tab: t } })

  // Console posture: one tab per work area, badge = where attention is needed.
  const unreadComms = channelsList.failed ? 0 : channels.reduce((n, c) => n + (c.unreadCount ?? 0), 0)
  const tabs: { id: HomeTab; label: string; badge?: number }[] = [
    { id: 'inbox', label: 'Inbox', badge: data?.unread || undefined },
    { id: 'boards', label: 'Boards', badge: data?.queues.triage.count || undefined },
    { id: 'comms', label: 'Comms', badge: unreadComms || undefined },
    { id: 'plans', label: 'Plans' },
    { id: 'research', label: 'Research' },
    { id: 'docs', label: 'Docs' },
    ...(isAdmin ? [{ id: 'fleet' as const, label: 'Fleet' }] : []),
  ]

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-6xl space-y-6">
        <h1 className="mercury-text text-2xl font-semibold">{greeting(session?.name ?? session?.email)}</h1>

        <Tabs
          value={tab}
          onChange={setTab}
          items={tabs.map((t) => ({
            id: t.id,
            label: (
              <span className="flex items-center gap-1.5">
                {t.label}
                {t.badge !== undefined && (
                  <span className="rounded-full bg-accent/15 px-1.5 text-[10px] font-semibold text-accent">{t.badge}</span>
                )}
              </span>
            ),
          }))}
        />
        {channelsList.notice}

        {tab === 'inbox' && (
          <div className="space-y-6">
            <AssistantBriefing />
            <NotificationsPanel />
            <ApprovalsPanel />
            <AgendaPanel />
            <MailPanel />
          </div>
        )}
        {tab === 'boards' && <BoardsTab home={home} />}
        {tab === 'comms' && <CommsTab />}
        {tab === 'plans' && <PlansTab />}
        {tab === 'research' && <ResearchTab />}
        {tab === 'docs' && <DocsTab />}
        {tab === 'fleet' && isAdmin && <FleetTab home={home} />}
      </div>
    </div>
  )
}

// ── Shared: an area-scoped slice of the workspace activity feed ─────────────
interface FeedEvent {
  at: string
  actor: string
  context: string
  detail: string
  href: string
}

function ActivityList({ kinds, title = 'Recent activity', collapsible = false }: { kinds: string[]; title?: string; collapsible?: boolean }) {
  const navigate = useNavigate()
  const [expanded, setExpanded] = useState(!collapsible)
  const query = useQuery({
    queryKey: ['home-activity', kinds.join(',')],
    queryFn: (): Promise<FeedEvent[]> => getList<FeedEvent>(`/api/activity?kinds=${kinds.join(',')}`, 'events'),
    refetchInterval: 30_000,
  })
  const { data, isLoading } = query
  return (
    <Panel>
      {collapsible ? (
        <button type="button" onClick={() => setExpanded((v) => !v)} className={cn('flex items-center gap-2 text-left', expanded && 'mb-3')}>
          {expanded ? <ChevronDown size={14} className="text-muted" /> : <ChevronRight size={14} className="text-muted" />}
          <span className="text-sm font-semibold text-fg">{title}</span>
        </button>
      ) : (
        <div className="mb-3 text-sm font-semibold text-fg">{title}</div>
      )}
      {!expanded ? null : isLoading ? (
        <SkeletonRows rows={6} />
      ) : data === undefined ? (
        <QueryError variant="inline" error={query.error} title="Could not load activity" onRetry={() => void query.refetch()} />
      ) : data.length === 0 ? (
        <div className="text-xs text-muted">Quiet so far.</div>
      ) : (
        <ul className="space-y-1">
          {data.slice(0, 12).map((a, i) => (
            <ActivityRow key={i} actor={a.actor} detail={a.detail} at={a.at} context={a.context} onClick={() => a.href && void navigate({ to: a.href })} />
          ))}
        </ul>
      )}
    </Panel>
  )
}

// ── Boards: board-scoped briefing, full queues behind a sidebar, audit trail ─
type QueueKey = 'triage' | 'review' | 'blocked'
const QUEUE_META: Record<QueueKey, { label: string; hint: string; accent: string }> = {
  triage: { label: 'To triage', hint: 'New tickets waiting to be assigned', accent: 'var(--theme-accent)' },
  review: { label: 'To review', hint: 'Agent work awaiting your sign-off', accent: 'var(--theme-success)' },
  blocked: { label: 'Blocked', hint: 'Stalled, needs you to unblock', accent: 'var(--theme-warning)' },
}

function BoardsTab({ home }: { home: UseQueryResult<HomeSummary> }) {
  const navigate = useNavigate()
  const [queue, setQueue] = useState<QueueKey>('triage')
  const { openMenu, menu } = useContextMenu()
  const meta = QUEUE_META[queue]
  const { data, isLoading } = home
  const items = data?.queues[queue].items ?? []
  return (
    <div className="space-y-6">
      <AssistantBriefing scope="boards" />
      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)]">
        <aside className="space-y-1">
          {(Object.keys(QUEUE_META) as QueueKey[]).map((k) => (
            <button
              key={k}
              type="button"
              onClick={() => setQueue(k)}
              className={cn(
                'flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm transition-colors',
                queue === k ? 'bg-card text-fg' : 'text-muted hover:bg-card hover:text-fg',
              )}
            >
              <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: QUEUE_META[k].accent }} />
              <span className="min-w-0 flex-1 truncate">{QUEUE_META[k].label}</span>
              {isLoading ? (
                <Skeleton className="h-3 w-6 rounded-full" />
              ) : (
                // A failed /api/home must not read as "0 waiting for you".
                <span className="shrink-0 text-xs text-muted">{data ? data.queues[k].count : '—'}</span>
              )}
            </button>
          ))}
        </aside>
        <Panel>
          <div className="mb-1 text-sm font-semibold text-fg">{meta.label}</div>
          <div className="mb-3 text-xs text-muted">{meta.hint}</div>
          {isLoading ? (
            <SkeletonRows rows={8} />
          ) : !data ? (
            // "All clear." is a statement about the queue. A broken /api/home
            // has no idea whether it's clear, so it must not say so.
            <QueryError
              variant="compact"
              error={home.error}
              title="Could not load your queues"
              onRetry={() => void home.refetch()}
            />
          ) : items.length === 0 ? (
            <div className="py-6 text-center text-xs text-muted">All clear.</div>
          ) : (
            <ul className="divide-y divide-line-subtle">
              {items.map((w) => (
                <li key={w.id}>
                  <button
                    type="button"
                    onClick={() => void navigate({ to: `/boards/${w.boardId}/${w.id}` })}
                    onContextMenu={(e) =>
                      openMenu(e, [
                        { label: 'Open', onSelect: () => void navigate({ to: `/boards/${w.boardId}/${w.id}` }) },
                        { label: 'Copy link', onSelect: () => copyAppLink(`/boards/${w.boardId}/${w.id}`) },
                        ...(w.ticketRef
                          ? ([
                              { label: 'Copy ticket ref', onSelect: () => void navigator.clipboard.writeText(w.ticketRef!) },
                            ] as ContextMenuEntry[])
                          : []),
                      ])
                    }
                    className="flex w-full items-center gap-3 py-2.5 text-left"
                  >
                    {w.ticketRef && <span className="shrink-0 font-[var(--font-mono)] text-[11px] text-muted">{w.ticketRef}</span>}
                    <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{w.title}</span>
                    <span className="shrink-0 text-xs text-muted">{w.board}</span>
                    <span className="shrink-0 text-xs text-muted">{relativeTime(w.updatedAt)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>
      <ActivityList kinds={['ticket']} title="Board activity" collapsible />
      {menu}
    </div>
  )
}

// ── Comms: what's unread, then the room's pulse ─────────────────────────────
function CommsTab() {
  const navigate = useNavigate()
  const { openMenu, menu } = useContextMenu()
  // "All caught up." over a 500 — on the landing surface, about messages.
  const { rows: channels, notice, failed, pending: isLoading } = listQuery(useChannels(), { title: 'Could not load your channels', variant: 'compact' })
  const unread = channels.filter((c) => (c.unreadCount ?? 0) > 0)
  const label = (c: (typeof channels)[number]) =>
    c.kind === 'dm' ? (c.peer?.name ?? c.peer?.email ?? 'DM') : `#${c.name}`
  return (
    <div className="space-y-6">
      <AssistantBriefing scope="comms" />
      <Panel>
        <div className="mb-3 text-sm font-semibold text-fg">Unread</div>
        {notice}
        {isLoading ? (
          <SkeletonRows rows={4} />
        ) : failed ? null : unread.length === 0 ? (
          <div className="text-xs text-muted">All caught up.</div>
        ) : (
          <ul className="divide-y divide-line-subtle">
            {unread.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  onClick={() => void navigate({ to: `/comms?c=${c.id}` })}
                  onContextMenu={(e) =>
                    openMenu(e, [
                      { label: 'Open', onSelect: () => void navigate({ to: `/comms?c=${c.id}` }) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/comms?c=${c.id}`) },
                    ])
                  }
                  className="flex w-full items-center gap-3 py-2.5 text-left"
                >
                  <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{label(c)}</span>
                  <span className="shrink-0 rounded-full bg-accent/15 px-2 py-0.5 text-xs font-semibold text-accent">{c.unreadCount}</span>
                  <span className="shrink-0 text-xs text-muted">{relativeTime(c.updatedAt)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
      <ActivityList kinds={['channel']} title="Channel activity" collapsible />
      {menu}
    </div>
  )
}

// ── Plans: your live plans (and ones shared with you) ───────────────────────
function PlansTab() {
  const navigate = useNavigate()
  const { openMenu, menu } = useContextMenu()
  // The original "No boards yet over a 500", verbatim, on Home: a failed read
  // renders "No plans yet. Start one on /plan." to an owner whose plans exist.
  const { rows: plans, notice, failed, pending: isLoading } = listQuery(useConversations('plan'), { title: 'Could not load your plans', variant: 'compact' })
  const { data: fleet } = useAgents()
  const agentLabel = (id: string) => fleet?.agents.find((a) => a.id === id)?.label ?? id
  return (
    <div className="space-y-6">
      <AssistantBriefing scope="plans" />
      <Panel>
        <div className="mb-3 text-sm font-semibold text-fg">Plans</div>
        {notice}
        {isLoading ? (
          <SkeletonRows rows={5} />
        ) : failed ? null : plans.length === 0 ? (
          <EmptyState variant="inline" title="No plans yet. Start one on /plan." />
        ) : (
          <ul className="divide-y divide-line-subtle">
            {plans.slice(0, 10).map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  onClick={() => void navigate({ to: `/plan?p=${c.id}` })}
                  onContextMenu={(e) =>
                    openMenu(e, [
                      { label: 'Open', onSelect: () => void navigate({ to: `/plan?p=${c.id}` }) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/plan?p=${c.id}`) },
                    ])
                  }
                  className="flex w-full items-center gap-3 py-2.5 text-left"
                >
                  {c.working && <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-accent" />}
                  <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{c.title || 'Untitled plan'}</span>
                  {c.failed && <span className="shrink-0 text-xs text-[color:var(--theme-danger)]">failed — open to retry →</span>}
                  {c.role === 'collaborator' && c.ownerLabel && (
                    <span className="shrink-0 text-[11px] text-muted">shared by {c.ownerLabel}</span>
                  )}
                  <span className="shrink-0 text-xs text-muted">{agentLabel(c.agentModel)}</span>
                  <span className="shrink-0 text-xs text-muted">{relativeTime(c.updatedAt)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
      {menu}
    </div>
  )
}

// ── Research: runs in flight and fresh reports ──────────────────────────────
const RUN_DOT: Record<string, string> = {
  queued: 'var(--theme-warning)',
  running: 'var(--theme-accent)',
  done: 'var(--theme-success)',
  error: 'var(--theme-danger)',
}

function ResearchTab() {
  const navigate = useNavigate()
  const { openMenu, menu } = useContextMenu()
  const { rows: runs, notice, failed, pending: isLoading } = listQuery(useResearchRuns(), { title: 'Could not load your research', variant: 'compact' })
  return (
    <div className="space-y-6">
      <AssistantBriefing scope="research" />
      <Panel>
        <div className="mb-3 text-sm font-semibold text-fg">Research</div>
        {notice}
        {isLoading ? (
          <SkeletonRows rows={5} />
        ) : failed ? null : runs.length === 0 ? (
          <div className="text-xs text-muted">Nothing researched yet. Ask something on /research.</div>
        ) : (
          <ul className="divide-y divide-line-subtle">
            {runs.slice(0, 10).map((r) => (
              <li key={r.id}>
                <button
                  type="button"
                  onClick={() => void navigate({ to: '/research', search: { r: r.id } })}
                  onContextMenu={(e) =>
                    openMenu(e, [
                      { label: 'Open', onSelect: () => void navigate({ to: '/research', search: { r: r.id } }) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/research?r=${r.id}`) },
                    ])
                  }
                  className="flex w-full items-center gap-3 py-2.5 text-left"
                >
                  <span className={cn('h-2 w-2 shrink-0 rounded-full', r.status === 'running' && 'animate-pulse')} style={{ background: RUN_DOT[r.status] ?? 'var(--theme-line)' }} />
                  <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{r.question}</span>
                  <span className="shrink-0 rounded border border-line-subtle px-1 text-[10px] uppercase tracking-wide text-muted">{r.mode}</span>
                  {r.status === 'error' ? (
                    <span className="shrink-0 text-xs text-[color:var(--theme-danger)]">failed — open to re-run →</span>
                  ) : (
                    <span className="shrink-0 text-xs text-muted">{r.status}</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
      {menu}
    </div>
  )
}

// ── Docs: the latest durable output (artifacts incl. official KB mirrors) ───
function DocsTab() {
  const navigate = useNavigate()
  const { openMenu, menu } = useContextMenu()
  const { rows: artifacts, notice, failed, pending: isLoading } = listQuery(useArtifacts(), { title: 'Could not load your documents', variant: 'compact' })
  const recent = [...artifacts].sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : -1)).slice(0, 12)
  return (
    <div className="space-y-6">
      <Panel>
        <div className="mb-3 text-sm font-semibold text-fg">Recently updated</div>
        {notice}
        {isLoading ? (
          <SkeletonRows rows={6} />
        ) : failed ? null : recent.length === 0 ? (
          <EmptyState variant="inline" title="No documents yet." />
        ) : (
          <ul className="divide-y divide-line-subtle">
            {recent.map((a) => (
              <li key={a.id}>
                <button
                  type="button"
                  onClick={() => void navigate({ to: '/artifacts', search: { a: a.id } })}
                  onContextMenu={(e) =>
                    openMenu(e, [
                      { label: 'Open', onSelect: () => void navigate({ to: '/artifacts', search: { a: a.id } }) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/artifacts?a=${a.id}`) },
                    ])
                  }
                  className="flex w-full items-center gap-3 py-2.5 text-left"
                >
                  <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{a.title}</span>
                  <span className="shrink-0 rounded border border-line-subtle px-1 text-[10px] uppercase tracking-wide text-muted">{a.kind}</span>
                  <span className="shrink-0 text-xs text-muted">{relativeTime(a.updatedAt)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
      {menu}
    </div>
  )
}

// ── Fleet (admin): status, alerts, spend, and the org pulse ─────────────────
function FleetTab({ home }: { home: UseQueryResult<HomeSummary> }) {
  const { data } = home
  // Resolved-with-nothing is impossible here (a 200 always carries the
  // summary), so an absent payload is either in flight or broken — and those
  // two must not look alike.
  if (!data && home.isError)
    return <QueryError error={home.error} title="Could not load fleet status" onRetry={() => void home.refetch()} />
  if (!data)
    return (
      <div className="grid gap-6 lg:grid-cols-2">
        <div className="space-y-4">
          <Panel>
            <SkeletonRows rows={2} />
          </Panel>
          <div className="grid grid-cols-2 gap-4">
            {[0, 1].map((i) => (
              <Panel key={i}>
                <Skeleton className="mb-3 h-2.5 w-14 rounded-full" delay={i * 0.1} />
                <Skeleton className="h-5 w-16 rounded-full" delay={i * 0.1 + 0.08} />
              </Panel>
            ))}
          </div>
        </div>
        <Panel>
          <Skeleton className="mb-4 h-3 w-12 rounded-full" />
          <SkeletonRows rows={8} />
        </Panel>
      </div>
    )
  const { org, fleet } = data
  return (
    <div className="grid gap-6 lg:grid-cols-2">
      <div className="space-y-4">
        <div className="px-1 text-[11px] uppercase tracking-wide text-muted">{org.name || 'Your org'}</div>
        <Panel>
          <div className="flex items-center gap-3">
            <span
              className="h-2.5 w-2.5 shrink-0 rounded-full"
              style={{ background: fleet.down.length ? 'var(--theme-warning)' : 'var(--theme-success)' }}
            />
            <span className="text-sm font-semibold text-fg">Fleet</span>
            <span className="min-w-0 flex-1 truncate text-sm text-muted">
              {fleet.online}/{fleet.total} online
              {fleet.down.length > 0 && ` · ${fleet.down.slice(0, 2).join(', ')} down`}
            </span>
            <Link to="/agents" className="shrink-0 text-xs text-accent hover:underline">
              Manage →
            </Link>
          </div>
        </Panel>
        <div className="grid grid-cols-2 gap-3">
          <Link to="/observability" search={{ tab: 'alerts' }} className="block">
            <Panel className="p-4">
              <div className="text-xs uppercase tracking-wide text-muted">Alerts</div>
              <div
                className="mt-1 text-xl font-semibold"
                style={{ color: (org.alerts ?? 0) > 0 ? 'var(--theme-warning)' : 'var(--theme-success)' }}
              >
                {org.alerts ?? 0}
              </div>
            </Panel>
          </Link>
          <Link to="/observability" search={{ tab: 'cost' }} className="block">
            <Panel className="p-4">
              <div className="text-xs uppercase tracking-wide text-muted">Spend today</div>
              <div className="mt-1 truncate text-xl font-semibold text-fg">
                {org.costToday ? `$${org.costToday.usd.toFixed(2)}` : '—'}
              </div>
              {org.costToday && (
                <div className="text-[11px] text-muted">{(org.costToday.tokens / 1000).toFixed(0)}k tokens</div>
              )}
            </Panel>
          </Link>
        </div>
        <ActivityList kinds={['fleet']} title="Fleet activity" />
      </div>
      <FleetPulse activity={org.activity} />
    </div>
  )
}

function FleetPulse({ activity }: { activity: OrgActivity[] }) {
  const navigate = useNavigate()
  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Pulse</div>
      {activity.length === 0 ? (
        <div className="text-xs text-muted">Quiet so far. Activity across boards, comms, and the fleet shows here.</div>
      ) : (
        <ul className="space-y-2">
          {activity.map((a, i) => (
            <ActivityRow key={i} actor={a.actor} detail={a.detail} at={a.at} context={a.context} onClick={() => a.href && void navigate({ to: a.href })} />
          ))}
        </ul>
      )}
    </Panel>
  )
}

// ── The assistant briefing: what needs you, in your assistant's voice ───────
// Regenerates only when the attention fingerprint changes; chatting back
// continues the same conversation through the normal chat pipeline.
interface BriefingData {
  none?: boolean
  summary: string
  agentModel: string | null
  agentName: string | null
  generating: boolean
  generatedAt: string | null
}

type BriefScope = 'inbox' | 'boards' | 'comms' | 'plans' | 'research'
const BRIEF_ASK: Record<BriefScope, string> = {
  inbox: 'about any of this',
  boards: 'about board work',
  comms: 'about your comms',
  plans: 'about your plans',
  research: 'about the research',
}

function AssistantBriefing({ scope = 'inbox' }: { scope?: BriefScope }) {
  const { data, isLoading } = useQuery({
    queryKey: ['briefing', scope],
    queryFn: (): Promise<BriefingData> => getJson<BriefingData>(`/api/me/briefing?scope=${scope}`),
    refetchInterval: (q) => (q.state.data?.generating ? 2_500 : 60_000),
  })
  const [thread, setThread] = useState<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  const [draft, setDraft] = useState('')
  const [replying, setReplying] = useState(false)

  const send = async () => {
    const content = draft.trim()
    if (!content || replying || !data?.agentModel) return
    const history = thread
    setDraft('')
    setThread((t) => [...t, { role: 'user', content }, { role: 'assistant', content: '' }])
    setReplying(true)
    try {
      // Ephemeral by design: streams through the assistant with the briefing
      // as context; nothing is persisted, the thread lives in this panel only.
      const res = await fetch('/api/me/briefing/chat', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ scope, content, history }),
      })
      if (!res.ok || !res.body) throw new Error(`chat failed: ${res.status}`)
      for await (const ev of parseAgentStream(res.body)) {
        if (ev.type === 'content') {
          setThread((t) => {
            const next = [...t]
            const last = next[next.length - 1]!
            next[next.length - 1] = { ...last, content: last.content + ev.text }
            return next
          })
        }
      }
    } catch {
      setThread((t) => [...t.slice(0, -1), { role: 'assistant', content: '(could not reply — try again)' }])
    } finally {
      setReplying(false)
    }
  }

  if (isLoading)
    return (
      <Panel>
        <div className="mb-3 flex items-center gap-2">
          <Skeleton className="h-5 w-5 rounded-full" />
          <Skeleton className="h-3 w-32 rounded-full" />
        </div>
        <SkeletonRows rows={3} />
      </Panel>
    )
  if (!data) return null
  if (data.none) return <AssistantCard />

  return (
    <Panel>
      <div className="mb-3 flex items-center gap-2">
        <Sparkles size={15} className="text-accent" />
        <span className="text-sm font-semibold text-fg">{data.agentName}</span>
        {data.generating ? (
          <span className="flex items-center gap-1.5 text-[11px] text-muted">
            <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent" /> updating
          </span>
        ) : (
          data.generatedAt && <span className="text-[11px] text-muted">{relativeTime(data.generatedAt)}</span>
        )}
      </div>
      {data.summary ? (
        <div className="text-sm">
          <Markdown>{data.summary}</Markdown>
        </div>
      ) : (
        <SkeletonRows rows={3} />
      )}
      {thread.length > 0 && (
        <div className="mt-4 space-y-3 border-t border-line-subtle pt-4">
          {thread.map((m, i) =>
            m.role === 'user' ? (
              <div key={i} className="flex justify-end">
                <div className="max-w-[85%] rounded-2xl border px-3.5 py-2 font-sans text-sm text-[color:var(--chat-user-foreground)]" style={{ background: 'var(--chat-user-bg)', borderColor: 'var(--chat-user-border)' }}>
                  {m.content}
                </div>
              </div>
            ) : (
              <div key={i} className="text-sm">
                {m.content ? <Markdown>{m.content}</Markdown> : <SkeletonRows rows={1} />}
              </div>
            ),
          )}
        </div>
      )}
      <div className="mt-4 flex items-end gap-2">
        <Textarea
          autoGrow
          rows={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void send()
            }
          }}
          placeholder={`Ask ${data.agentName} ${BRIEF_ASK[scope]}`}
          className="max-h-32 text-sm"
        />
      </div>
    </Panel>
  )
}

// The personal assistant card: everyone can set up their own agent (its own
// container, memory, key) through the onboarding wizard, then jump into chat.
function AssistantCard() {
  const navigate = useNavigate()
  const [wizard, setWizard] = useState(false)
  const { data, isLoading } = useAssistant()

  if (isLoading)
    return (
      <Panel className="flex items-center gap-4">
        <Skeleton className="h-11 w-11 shrink-0 rounded-2xl" />
        <div className="min-w-0 flex-1 space-y-2">
          <Skeleton className="h-3 w-40 rounded-full" />
          <Skeleton className="h-2.5 w-64 rounded-full" />
        </div>
      </Panel>
    )
  return (
    <Panel className="flex items-center gap-4">
      <span className="grid h-11 w-11 shrink-0 place-items-center rounded-2xl bg-accent/15 text-accent">
        <Sparkles size={20} />
      </span>
      {data ? (
        <>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-fg">{data.displayName}</div>
            <div className="truncate text-xs text-muted">Your personal assistant, with its own memory, skills, and tools.</div>
          </div>
          <Button size="sm" onClick={() => void navigate({ to: '/chat' })}>
            Open chat
          </Button>
        </>
      ) : (
        <>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-fg">Set up your assistant</div>
            <div className="truncate text-xs text-muted">A personal agent that's just yours: memory, skills, and tools of its own.</div>
          </div>
          <Button size="sm" onClick={() => setWizard(true)}>
            Get started
          </Button>
        </>
      )}
      {wizard && <AssistantWizard onClose={() => setWizard(false)} />}
    </Panel>
  )
}



interface AgendaEvent {
  id: string
  summary: string
  start: string | null
  end: string | null
  allDay: boolean
  location: string | null
  htmlLink: string | null
}

// The user's Google Calendar agenda, shown only when they've connected Google.
// Stays invisible otherwise so Home isn't cluttered for the unconnected.
function AgendaPanel() {
  const qc = useQueryClient()
  const { data, isError } = useQuery({
    queryKey: ['agenda'],
    queryFn: async (): Promise<{ events?: AgendaEvent[]; error?: string }> => {
      const r = await fetch('/api/integrations/google/calendar/events')
      // 409 (not connected) and 502 (Google hiccup) are ANSWERS this panel
      // knows how to render — it hides. Every other non-2xx is a failure.
      if (r.status === 409 || r.status === 502) return { error: 'unavailable' }
      return readJson<{ events?: AgendaEvent[] }>(r)
    },
    retry: false,
    refetchInterval: 5 * 60_000,
  })
  const [adding, setAdding] = useState(false)

  if (!data && !isError)
    return (
      <Panel>
        <Skeleton className="mb-4 h-3 w-20 rounded-full" />
        <SkeletonRows rows={3} />
      </Panel>
    )
  // Not connected (or unreachable) → render nothing.
  if (isError || data?.error || !data) return null
  const events = data.events ?? []

  return (
    <Panel>
      <div className="mb-3 flex items-center gap-2">
        <CalendarDays size={16} className="text-muted" />
        <span className="text-sm font-semibold text-fg">Agenda</span>
        <span className="text-xs text-muted">Google Calendar</span>
        <button type="button" onClick={() => setAdding((v) => !v)} className="ml-auto flex items-center gap-1 text-xs text-accent hover:underline">
          <Plus size={13} /> New event
        </button>
      </div>

      {adding && <QuickEvent onDone={async () => { setAdding(false); await qc.invalidateQueries({ queryKey: ['agenda'] }) }} />}

      {events.length === 0 ? (
        <div className="py-3 text-sm text-muted">Nothing on the calendar coming up.</div>
      ) : (
        <div className="divide-y divide-line-subtle">
          {events.map((e) => (
            <a
              key={e.id}
              href={e.htmlLink ?? '#'}
              target="_blank"
              rel="noreferrer"
              className="group flex items-center gap-3 py-2"
            >
              <span className="w-32 shrink-0 text-[11px] text-muted">{formatWhen(e)}</span>
              <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{e.summary}</span>
              {e.location && <span className="hidden shrink-0 truncate text-[11px] text-muted sm:block sm:max-w-[8rem]">{e.location}</span>}
              <ExternalLink size={12} className="shrink-0 text-muted opacity-0 group-hover:opacity-100" />
            </a>
          ))}
        </div>
      )}
    </Panel>
  )
}

function formatWhen(e: AgendaEvent): string {
  if (!e.start) return ''
  const d = new Date(e.start)
  if (e.allDay) return d.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' }) + ' · all day'
  return d.toLocaleString(undefined, { weekday: 'short', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
}

// Minimal create form: title + start; end defaults to +1h.
function QuickEvent({ onDone }: { onDone: () => void }) {
  const [summary, setSummary] = useState('')
  const [start, setStart] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const submit = async () => {
    if (!summary.trim() || !start) return
    setBusy(true)
    setErr(null)
    try {
      const startISO = new Date(start).toISOString()
      const endISO = new Date(new Date(start).getTime() + 60 * 60_000).toISOString()
      const r = await fetch('/api/integrations/google/calendar/events', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ summary: summary.trim(), start: startISO, end: endISO }),
      })
      const j = (await r.json().catch(() => null)) as { event?: unknown; message?: string } | null
      if (r.ok && j?.event) onDone()
      else setErr(j?.message ?? 'Could not create the event.')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mb-3 flex flex-wrap items-center gap-2 rounded-xl border border-line-subtle p-2">
      <Input
        size="sm"
        value={summary}
        onChange={(e) => setSummary(e.target.value)}
        placeholder="Event title"
        className="min-w-0 flex-1"
      />
      <Input
        size="sm"
        type="datetime-local"
        value={start}
        onChange={(e) => setStart(e.target.value)}
        className="w-auto"
      />
      <Button size="sm" onClick={() => void submit()} disabled={busy || !summary.trim() || !start}>
        Add
      </Button>
      {err && <span className="w-full text-[11px]" style={{ color: 'var(--theme-danger)' }}>{err}</span>}
    </div>
  )
}

interface Mail {
  id: string
  threadId: string
  from: string
  subject: string
  snippet: string
  date: string | null
  unread: boolean
}

// Recent Gmail, shown only when the user has connected Google. Compose sends as
// the user via Gmail.
function MailPanel() {
  const { data, isError } = useQuery({
    queryKey: ['gmail'],
    queryFn: async (): Promise<{ messages?: Mail[]; error?: string }> => {
      const r = await fetch('/api/integrations/google/gmail/messages')
      // Same contract as Agenda: 409 / 502 mean "nothing to show here".
      if (r.status === 409 || r.status === 502) return { error: 'unavailable' }
      return readJson<{ messages?: Mail[] }>(r)
    },
    retry: false,
    refetchInterval: 5 * 60_000,
  })
  const [composing, setComposing] = useState(false)

  // In flight → hold the space with a skeleton; only a RESOLVED
  // not-connected state may remove the panel (no pop-in, no dead panel).
  if (!data && !isError)
    return (
      <Panel>
        <Skeleton className="mb-4 h-3 w-16 rounded-full" />
        <SkeletonRows rows={4} />
      </Panel>
    )
  if (isError || data?.error || !data) return null
  const messages = data.messages ?? []

  const fromName = (from: string) => from.replace(/<[^>]*>/, '').replace(/"/g, '').trim() || from

  return (
    <Panel>
      <div className="mb-3 flex items-center gap-2">
        <Mail size={16} className="text-muted" />
        <span className="text-sm font-semibold text-fg">Mail</span>
        <span className="text-xs text-muted">Gmail</span>
        <button type="button" onClick={() => setComposing(true)} className="ml-auto flex items-center gap-1 text-xs text-accent hover:underline">
          <Send size={12} /> Compose
        </button>
      </div>

      {messages.length === 0 ? (
        <div className="py-3 text-sm text-muted">No recent mail.</div>
      ) : (
        <div className="divide-y divide-line-subtle">
          {messages.map((m) => (
            <a
              key={m.id}
              href={`https://mail.google.com/mail/u/0/#all/${m.threadId}`}
              target="_blank"
              rel="noreferrer"
              className="group flex items-center gap-3 py-2"
            >
              <span className={cn('w-32 shrink-0 truncate font-sans text-[12px]', m.unread ? 'font-semibold text-fg' : 'text-muted')}>{fromName(m.from)}</span>
              <span className="min-w-0 flex-1 truncate font-sans text-sm">
                <span className={m.unread ? 'font-medium text-fg' : 'text-fg'}>{m.subject}</span>
                <span className="text-muted"> — {m.snippet}</span>
              </span>
              <span className="shrink-0 text-[11px] text-muted">{m.date ? relativeTime(m.date) : ''}</span>
            </a>
          ))}
        </div>
      )}

      {composing && <ComposeModal onClose={() => setComposing(false)} />}
    </Panel>
  )
}

function ComposeModal({ onClose }: { onClose: () => void }) {
  const [to, setTo] = useState('')
  const [subject, setSubject] = useState('')
  const [body, setBody] = useState('')
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)

  const send = async () => {
    if (!to.trim()) return
    setBusy(true)
    setStatus(null)
    try {
      const r = await fetch('/api/integrations/google/gmail/send', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ to: to.trim(), subject, body }),
      })
      const j = (await r.json().catch(() => null)) as { sent?: unknown; message?: string } | null
      if (r.ok && j?.sent) onClose()
      else setStatus(j?.message ?? 'Could not send the email.')
    } finally {
      setBusy(false)
    }
  }

  // The shared Modal (Escape + backdrop close) with the shared field
  // primitives — this was the one dialog still hand-rolling both.
  return (
    <Modal open onClose={onClose} title={<span className="flex items-center gap-2"><Send size={15} className="text-muted" /> New message</span>} width="max-w-lg">
      <div className="space-y-2">
        <Input autoFocus value={to} onChange={(e) => setTo(e.target.value)} placeholder="To" />
        <Input value={subject} onChange={(e) => setSubject(e.target.value)} placeholder="Subject" />
        <Textarea value={body} onChange={(e) => setBody(e.target.value)} placeholder="Write your message" rows={8} className="w-full resize-y" />
        <div className="flex items-center gap-3">
          <Button size="sm" onClick={() => void send()} disabled={busy || !to.trim()}>
            <Send size={13} className="mr-1" /> {busy ? 'Sending' : 'Send'}
          </Button>
          {status && <span className="text-xs" style={{ color: 'var(--theme-danger)' }}>{status}</span>}
        </div>
      </div>
    </Modal>
  )
}

interface PendingAction {
  id: string
  kind: string
  summary: string | null
  agentModel: string | null
  isOrg: boolean
  createdAt: string
}

// Agent-drafted Google actions (send email / create event) awaiting the user's
// approval — confirm-sends. Hidden when there's nothing to approve.
function ApprovalsPanel() {
  const qc = useQueryClient()
  const query = useQuery({
    queryKey: ['google-pending'],
    // An agent waiting to act AS YOU is the last thing that may go quiet on a
    // 500 — this queue must never be emptied by a failed read.
    // `getList`, not `getJson<{pending}>`: a 200 whose `pending` key is missing
    // is a BROKEN CONTRACT, and `getJson` handed that body straight through, so
    // `data.pending.length` two lines down threw and took the whole Home route
    // to the catch boundary — the landing surface replaced by "This view failed
    // to load". getList refuses the body instead, which lands in the error
    // branch this panel already has. Same rule as every other list read.
    queryFn: (): Promise<PendingAction[]> => getList<PendingAction>('/api/integrations/google/pending', 'pending'),
    refetchInterval: 60_000,
  })
  const { data, isError } = query
  const [busy, setBusy] = useState<string | null>(null)
  // In flight → hold the space (same pattern as Agenda/Mail below); only a
  // RESOLVED empty queue may remove the panel, so it never drops in above the
  // queues and shifts the page.
  if (!data && !isError)
    return (
      <Panel>
        <Skeleton className="mb-4 h-3 w-36 rounded-full" />
        <SkeletonRows rows={2} />
      </Panel>
    )
  // Broken ≠ nothing to approve: say so instead of disappearing.
  if (!data)
    return (
      <Panel>
        <QueryError
          variant="inline"
          error={query.error}
          title="Could not check for approvals"
          onRetry={() => void query.refetch()}
        />
      </Panel>
    )
  const pending = data
  if (pending.length === 0) return null

  const decide = async (id: string, decision: 'approve' | 'reject') => {
    setBusy(id)
    try {
      const r = await fetch(`/api/integrations/google/pending/${id}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ decision }),
      })
      if (!r.ok) {
        const j = (await r.json().catch(() => null)) as { message?: string } | null
        await alert({ title: "Couldn't complete", message: j?.message ?? 'Could not complete that action.' })
      }
      await qc.invalidateQueries({ queryKey: ['google-pending'] })
      await qc.invalidateQueries({ queryKey: ['agenda'] })
      await qc.invalidateQueries({ queryKey: ['gmail'] })
    } finally {
      setBusy(null)
    }
  }

  const kindLabel = (k: string) => (k === 'gmail_send' ? 'Send email' : k === 'calendar_create' ? 'Create event' : k)

  return (
    <Panel>
      <div className="mb-3 flex items-center gap-2">
        <ShieldCheck size={16} style={{ color: 'var(--theme-warning)' }} />
        <span className="text-sm font-semibold text-fg">Needs your approval</span>
        <span className="text-xs text-muted">an agent wants to act as you</span>
      </div>
      <div className="divide-y divide-line-subtle">
        {pending.map((a) => (
          <div key={a.id} className="flex items-center gap-3 py-2.5">
            <Chip>{kindLabel(a.kind)}</Chip>
            {a.isOrg && <span className="shrink-0 rounded bg-card px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted" title="Shared org account">org</span>}
            <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{a.summary ?? '(action)'}</span>
            {a.agentModel && <span className="hidden shrink-0 text-[11px] text-muted sm:block">{a.agentModel}</span>}
            <div className="flex shrink-0 items-center gap-1">
              <Button size="sm" disabled={busy === a.id} onClick={() => void decide(a.id, 'approve')}>
                <Check size={13} className="mr-1" /> Approve
              </Button>
              <Button variant="ghost" size="sm" disabled={busy === a.id} onClick={() => void decide(a.id, 'reject')}>
                Reject
              </Button>
            </div>
          </div>
        ))}
      </div>
    </Panel>
  )
}

import { createFileRoute, Link, useNavigate } from '@tanstack/react-router'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { MessageSquare, Hash, LayoutGrid, Inbox as InboxIcon, Sparkles, CalendarDays, Plus, ExternalLink, Mail, Send, X, ShieldCheck, Check } from 'lucide-react'
import { Panel } from '@/components/ui/panel'
import { alert } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { AssistantWizard } from '@/components/assistant/assistant-wizard'
import { NotificationsPanel } from '@/components/app/notifications-panel'
import { relativeTime } from '@/lib/fleet'
import { cn } from '@/lib/cn'
import { useAssistant } from '@/lib/assistant'
import { useSession } from '@/lib/session'

export const Route = createFileRoute('/_app/')({
  component: HomePage,
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
    queryFn: async (): Promise<HomeSummary> => {
      const r = await fetch('/api/home')
      if (!r.ok) throw new Error('failed to load')
      return r.json()
    },
    refetchInterval: 30_000,
  })

const greeting = (name?: string | null) => {
  const who = name?.split(' ')[0] ?? name ?? 'there'
  return `Welcome back, ${who}`
}

// Home/Today — the seamless landing. Surfaces the human's real job in Talaria's
// guardrail model (triage · review · unblock), unread mentions, fleet health,
// and one-tap entries into the work surfaces.
function HomePage() {
  const { data: session } = useSession()
  const { data, isLoading } = useHome()
  const navigate = useNavigate()

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-6xl space-y-6">
        <div>
          <h1 className="mercury-text text-2xl font-semibold">{greeting(session?.name ?? session?.email)}</h1>
          <p className="mt-1 text-sm text-muted">Your inbox: what needs you, and how the org is doing.</p>
        </div>

        {/* Two zones: YOURS (left) — everything waiting on you — and THE ORG
            (right rail) — the shared at-a-glance. */}
        <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_21rem]">
          <div className="min-w-0 space-y-6">
            {/* Notifications first — this IS the inbox now. */}
            <NotificationsPanel />

            <ApprovalsPanel />

            {isLoading ? (
              <div className="text-sm text-muted">Loading your day</div>
            ) : (
              <div className="grid gap-4 xl:grid-cols-3">
                <QueuePanel
                  title="To triage"
                  hint="New tickets waiting to be assigned"
                  queue={data!.queues.triage}
                  accent="var(--theme-accent)"
                  onOpen={(w) => void navigate({ to: `/boards/${w.boardId}/${w.id}` })}
                />
                <QueuePanel
                  title="To review"
                  hint="Agent work awaiting your sign-off"
                  queue={data!.queues.review}
                  accent="var(--theme-success)"
                  onOpen={(w) => void navigate({ to: `/boards/${w.boardId}/${w.id}` })}
                />
                <QueuePanel
                  title="Blocked"
                  hint="Stalled — needs you to unblock"
                  queue={data!.queues.blocked}
                  accent="var(--theme-warning)"
                  onOpen={(w) => void navigate({ to: `/boards/${w.boardId}/${w.id}` })}
                />
              </div>
            )}

            <AgendaPanel />

            <MailPanel />

            {/* Quick entries into the work surfaces */}
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <QuickCard to="/comms" icon={<MessageSquare size={18} />} label="Comms" sub="Channels · relays · DMs" />
              <QuickCard to="/plan" icon={<Hash size={18} />} label="Plan" sub="Think, then ticket" />
              <QuickCard to="/boards" icon={<LayoutGrid size={18} />} label="Boards" sub="Move work" />
              <QuickCard to="/artifacts" icon={<InboxIcon size={18} />} label="Artifacts" sub="Docs · files · memes" />
            </div>

            <AssistantCard />
          </div>

          <OrgRail data={data} isAdmin={session?.role === 'admin'} />
        </div>
      </div>
    </div>
  )
}

// The org's at-a-glance rail: fleet health, a live activity pulse everyone
// sees, and — for admins — alerts and today's spend. The personal column is
// "what needs me"; this is "how are we doing."
function OrgRail({ data, isAdmin }: { data: HomeSummary | undefined; isAdmin: boolean }) {
  const navigate = useNavigate()
  if (!data) return <div className="hidden lg:block" />
  const { org, fleet } = data
  return (
    <aside className="space-y-4">
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
          {isAdmin && (
            <Link to="/agents" className="shrink-0 text-xs text-accent hover:underline">
              Manage →
            </Link>
          )}
        </div>
      </Panel>

      {isAdmin && (
        <div className="grid grid-cols-2 gap-3">
          <Link to="/alerts" className="block">
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
          <Link to="/cost" className="block">
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
      )}

      <Panel>
        <div className="mb-2 text-sm font-semibold text-fg">Pulse</div>
        {org.activity.length === 0 ? (
          <div className="text-xs text-muted">Quiet so far — activity across boards, comms, and the fleet shows here.</div>
        ) : (
          <ul className="space-y-2">
            {org.activity.map((a, i) => (
              <li key={i}>
                <button
                  type="button"
                  onClick={() => a.href && void navigate({ to: a.href })}
                  className="w-full rounded-lg px-1.5 py-1 text-left transition-colors hover:bg-card"
                >
                  <div className="flex items-baseline gap-2">
                    <span className="min-w-0 flex-1 truncate text-xs text-fg">
                      <span className="font-medium">{a.actor}</span> · {a.detail}
                    </span>
                    <span className="shrink-0 text-[10px] text-muted">{relativeTime(a.at)}</span>
                  </div>
                  <div className="truncate pl-0 text-[11px] text-muted">{a.context}</div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </aside>
  )
}

// The personal assistant card: everyone can set up their own agent (its own
// container, memory, key) through the onboarding wizard, then jump into chat.
function AssistantCard() {
  const navigate = useNavigate()
  const [wizard, setWizard] = useState(false)
  const { data, isLoading } = useAssistant()

  if (isLoading) return null
  return (
    <Panel className="flex items-center gap-4">
      <span className="grid h-11 w-11 shrink-0 place-items-center rounded-2xl bg-accent/15 text-accent">
        <Sparkles size={20} />
      </span>
      {data ? (
        <>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-fg">{data.displayName}</div>
            <div className="truncate text-xs text-muted">Your personal assistant — its own memory, skills, and tools.</div>
          </div>
          <Button size="sm" onClick={() => void navigate({ to: '/chat' })}>
            Open chat
          </Button>
        </>
      ) : (
        <>
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-fg">Set up your assistant</div>
            <div className="truncate text-xs text-muted">A personal agent that's just yours — memory, skills, and tools of its own.</div>
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

function QuickCard({ to, icon, label, sub, badge }: { to: string; icon: React.ReactNode; label: string; sub: string; badge?: number }) {
  return (
    <Link
      to={to}
      className="group relative flex flex-col gap-2 rounded-2xl border border-line-subtle bg-card/40 p-4 transition-colors hover:border-accent hover:bg-card"
    >
      <span className="text-accent">{icon}</span>
      <div>
        <div className="text-sm font-medium text-fg">{label}</div>
        <div className="truncate text-xs text-muted">{sub}</div>
      </div>
      {badge ? (
        <span className="absolute right-3 top-3 rounded-full bg-accent px-1.5 text-[10px] font-semibold text-surface">{badge}</span>
      ) : null}
    </Link>
  )
}

function QueuePanel({
  title,
  hint,
  queue,
  accent,
  onOpen,
}: {
  title: string
  hint: string
  queue: Queue
  accent: string
  onOpen: (w: WorkItem) => void
}) {
  return (
    <Panel className="flex min-h-[12rem] flex-col">
      <div className="mb-1 flex items-baseline gap-2">
        <span className="text-sm font-semibold text-fg">{title}</span>
        <span className="rounded-full px-1.5 text-xs font-semibold" style={{ background: `color-mix(in srgb, ${accent} 18%, transparent)`, color: accent }}>
          {queue.count}
        </span>
      </div>
      <p className="mb-3 text-xs text-muted">{hint}</p>
      {queue.items.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState icon="✓" title="All clear" />
        </div>
      ) : (
        <div className="-mx-2 divide-y divide-line-subtle">
          {queue.items.map((w) => (
            <button
              key={w.id}
              type="button"
              onClick={() => onOpen(w)}
              className="flex w-full items-baseline gap-2 rounded-lg px-2 py-2 text-left transition-colors hover:bg-card"
            >
              {w.ticketRef && <span className="shrink-0 font-[var(--font-mono)] text-[11px] text-muted">{w.ticketRef}</span>}
              <span className="min-w-0 flex-1 truncate text-sm text-fg">{w.title}</span>
              <span className="shrink-0 text-[11px] text-muted">{relativeTime(w.updatedAt)}</span>
            </button>
          ))}
          {queue.count > queue.items.length && (
            <div className="px-2 pt-2 text-[11px] text-muted">+{queue.count - queue.items.length} more</div>
          )}
        </div>
      )}
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
      if (r.status === 409 || r.status === 502) return { error: 'unavailable' } // not connected / transient
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    retry: false,
    refetchInterval: 5 * 60_000,
  })
  const [adding, setAdding] = useState(false)

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
              <span className="min-w-0 flex-1 truncate text-sm text-fg">{e.summary}</span>
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
      <input
        value={summary}
        onChange={(e) => setSummary(e.target.value)}
        placeholder="Event title"
        className="min-w-0 flex-1 bg-transparent px-2 text-sm text-fg outline-none placeholder:text-muted"
      />
      <input
        type="datetime-local"
        value={start}
        onChange={(e) => setStart(e.target.value)}
        className="rounded-lg border border-line-subtle bg-transparent px-2 py-1 text-xs text-fg outline-none"
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
      if (r.status === 409 || r.status === 502) return { error: 'unavailable' }
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    retry: false,
    refetchInterval: 5 * 60_000,
  })
  const [composing, setComposing] = useState(false)

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
              <span className={cn('w-32 shrink-0 truncate text-[12px]', m.unread ? 'font-semibold text-fg' : 'text-muted')}>{fromName(m.from)}</span>
              <span className="min-w-0 flex-1 truncate text-sm">
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

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[8vh]" onClick={onClose}>
      <div className="w-full max-w-lg rounded-2xl border border-line bg-card shadow-xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-3">
          <Send size={15} className="text-muted" />
          <span className="text-sm font-semibold text-fg">New message</span>
          <button type="button" onClick={onClose} className="ml-auto rounded p-1 text-muted hover:text-fg"><X size={15} /></button>
        </div>
        <div className="space-y-2 p-4">
          <input value={to} onChange={(e) => setTo(e.target.value)} placeholder="To" className="w-full rounded-lg border border-line-subtle bg-transparent px-3 py-2 text-sm text-fg outline-none placeholder:text-muted" />
          <input value={subject} onChange={(e) => setSubject(e.target.value)} placeholder="Subject" className="w-full rounded-lg border border-line-subtle bg-transparent px-3 py-2 text-sm text-fg outline-none placeholder:text-muted" />
          <textarea value={body} onChange={(e) => setBody(e.target.value)} placeholder="Write your message" rows={8} className="w-full resize-y rounded-lg border border-line-subtle bg-transparent px-3 py-2 text-sm text-fg outline-none placeholder:text-muted" />
          <div className="flex items-center gap-3">
            <Button size="sm" onClick={() => void send()} disabled={busy || !to.trim()}>
              <Send size={13} className="mr-1" /> {busy ? 'Sending' : 'Send'}
            </Button>
            {status && <span className="text-xs" style={{ color: 'var(--theme-danger)' }}>{status}</span>}
          </div>
        </div>
      </div>
    </div>
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
  const { data } = useQuery({
    queryKey: ['google-pending'],
    queryFn: async (): Promise<{ pending: PendingAction[] }> => {
      const r = await fetch('/api/integrations/google/pending')
      if (!r.ok) return { pending: [] }
      return r.json()
    },
    refetchInterval: 60_000,
  })
  const [busy, setBusy] = useState<string | null>(null)
  const pending = data?.pending ?? []
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
            <span className="shrink-0 rounded border border-line-subtle px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted">{kindLabel(a.kind)}</span>
            {a.isOrg && <span className="shrink-0 rounded bg-card px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted" title="Shared org account">org</span>}
            <span className="min-w-0 flex-1 truncate text-sm text-fg">{a.summary ?? '(action)'}</span>
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

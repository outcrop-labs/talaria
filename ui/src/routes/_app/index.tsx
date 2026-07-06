import { createFileRoute, Link, useNavigate } from '@tanstack/react-router'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { MessageSquare, Hash, LayoutGrid, Inbox as InboxIcon, Sparkles } from 'lucide-react'
import { Panel } from '@/components/ui/panel'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { relativeTime } from '@/lib/fleet'
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
interface HomeSummary {
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
      <div className="mx-auto max-w-5xl space-y-8">
        <div>
          <h1 className="mercury-text text-2xl font-semibold">{greeting(session?.name ?? session?.email)}</h1>
          <p className="mt-1 text-sm text-muted">Here's what needs you, and where the fleet stands.</p>
        </div>

        <AssistantCard />

        {/* Quick entries into the work surfaces */}
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <QuickCard to="/chat" icon={<MessageSquare size={18} />} label="Chat" sub="Talk to an agent" />
          <QuickCard to="/channels" icon={<Hash size={18} />} label="Channels" sub="Team + agents" />
          <QuickCard to="/boards" icon={<LayoutGrid size={18} />} label="Boards" sub="Move work" />
          <QuickCard to="/inbox" icon={<InboxIcon size={18} />} label="Inbox" sub={data?.unread ? `${data.unread} unread` : 'Mentions'} badge={data?.unread} />
        </div>

        {isLoading ? (
          <div className="text-sm text-muted">Loading your day…</div>
        ) : (
          <>
            {/* The human's queues: triage, review, unblock */}
            <div className="grid gap-4 lg:grid-cols-3">
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

            {/* Fleet health glance */}
            <Panel>
              <div className="flex items-center gap-3">
                <span
                  className="h-2.5 w-2.5 shrink-0 rounded-full"
                  style={{ background: data!.fleet.down.length ? 'var(--theme-warning)' : 'var(--theme-success)' }}
                />
                <span className="text-sm font-semibold text-fg">Fleet</span>
                <span className="text-sm text-muted">
                  {data!.fleet.online}/{data!.fleet.total} agents online
                  {data!.fleet.down.length > 0 && ` · ${data!.fleet.down.slice(0, 3).join(', ')} offline`}
                </span>
                <Link to="/agents" className="ml-auto text-xs text-accent hover:underline">
                  Manage →
                </Link>
              </div>
            </Panel>
          </>
        )}
      </div>
    </div>
  )
}

interface Assistant {
  id: string
  slug: string
  model: string
  displayName: string
  enabled: boolean
}

// The one-button personal assistant: everyone can spin up their own agent
// (its own container, memory, key) and jump straight into a chat with it.
function AssistantCard() {
  const qc = useQueryClient()
  const navigate = useNavigate()
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const { data, isLoading } = useQuery({
    queryKey: ['my-assistant'],
    queryFn: async (): Promise<Assistant | null> => {
      const r = await fetch('/api/me/assistant')
      if (!r.ok) return null
      return ((await r.json()) as { assistant: Assistant | null }).assistant
    },
  })

  const create = async () => {
    setBusy(true)
    setErr(null)
    try {
      const r = await fetch('/api/me/assistant', { method: 'POST' })
      const j = (await r.json()) as { assistant?: Assistant; error?: string }
      if (!r.ok || j.error) setErr(j.error ?? 'could not create your assistant')
      else {
        await qc.invalidateQueries({ queryKey: ['my-assistant'] })
        await qc.invalidateQueries({ queryKey: ['agents'] })
      }
    } finally {
      setBusy(false)
    }
  }

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
            <div className="text-sm font-medium text-fg">Spin up your assistant</div>
            <div className="truncate text-xs text-muted">A personal agent that's just yours — memory, skills, and tools of its own.</div>
            {err && <div className="mt-1 text-xs" style={{ color: 'var(--theme-danger)' }}>{err}</div>}
          </div>
          <Button size="sm" onClick={() => void create()} disabled={busy}>
            {busy ? 'Creating…' : 'Create assistant'}
          </Button>
        </>
      )}
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

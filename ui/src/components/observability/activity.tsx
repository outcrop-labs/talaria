import { useNavigate } from '@tanstack/react-router'
import { SkeletonRows } from '@/components/ui/skeleton'
import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { Chip } from '@/components/ui/chip'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { QueryError } from '@/components/ui/query-state'
import { getList } from '@/lib/fetch-json'
import { relativeTime } from '@/lib/fleet'
import { useSession } from '@/lib/session'

type Kind = 'ticket' | 'channel' | 'fleet' | 'audit'

interface ActivityEvent {
  at: string
  kind: Kind
  actor: string
  context: string
  detail: string
  type: string
  href: string
}

const KIND_META: Record<Kind, { label: string; icon: string; blurb: string }> = {
  ticket: { label: 'Tickets', icon: '⧉', blurb: 'Board activity — status moves, dispatches, comments, gaps.' },
  channel: { label: 'Channels', icon: '⋕', blurb: 'Messages in channels you belong to.' },
  fleet: { label: 'Fleet', icon: '◍', blurb: 'Agent configuration versions.' },
  audit: { label: 'Governance', icon: '⛨', blurb: 'Admin actions — settings, permissions, renders, deletions.' },
}

// The event's own type — the second-level answer to "where did this come
// from" within a source. Warn-tinted for the ones worth a second look.
const WARN_TYPES = new Set(['gap', 'blocked'])

function useActivity(kinds: Kind[]) {
  return useQuery({
    queryKey: ['activity', kinds.join(',')],
    queryFn: (): Promise<ActivityEvent[]> =>
      getList<ActivityEvent>(`/api/activity${kinds.length ? `?kinds=${kinds.join(',')}` : ''}`, 'events'),
    refetchInterval: 30_000,
  })
}

// Everything that happened across the workspace, organized by WHERE it came
// from: one section per source (tickets, channels, fleet, governance), each
// row labeled with its own event type. Chips narrow to the sources you care
// about; governance is admin-only and off by default.
export function AuditPanel() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const [kinds, setKinds] = useState<Kind[]>([])
  const query = useActivity(kinds)
  const events = query.data ?? []
  // Stale-but-real beats blank: only a failure with nothing to fall back on
  // takes the feed over. "Nothing yet" may only come from a 200.
  const failed = query.isError && query.data === undefined
  const navigate = useNavigate()

  const available = (Object.keys(KIND_META) as Kind[]).filter((k) => k !== 'audit' || isAdmin)
  const active = kinds.length ? kinds : available.filter((k) => k !== 'audit')
  const toggle = (k: Kind) =>
    setKinds((prev) => {
      const cur = prev.length ? prev : active
      return cur.includes(k) ? cur.filter((x) => x !== k) : [...cur, k]
    })

  const byKind = new Map<Kind, ActivityEvent[]>()
  for (const e of events) {
    const list = byKind.get(e.kind) ?? []
    list.push(e)
    byKind.set(e.kind, list)
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-1.5">
        <div className="ml-auto flex gap-1.5">
          {available.map((k) => {
            const on = active.includes(k)
            return (
              <button
                key={k}
                type="button"
                title={KIND_META[k].blurb}
                onClick={() => toggle(k)}
                className={cn(
                  'rounded-full border px-3 py-1 text-xs transition-colors',
                  on ? 'border-accent text-fg' : 'border-line-subtle text-muted hover:text-fg',
                )}
              >
                {KIND_META[k].icon} {KIND_META[k].label}
              </button>
            )
          })}
        </div>
      </div>

      {failed ? (
        <QueryError error={query.error} title="Could not load activity" onRetry={() => void query.refetch()} />
      ) : query.isLoading ? (
        <SkeletonRows rows={8} avatar />
      ) : events.length === 0 ? (
        <EmptyState
          icon="☰"
          title="Nothing yet"
          hint="Ticket updates, channel messages, agent config changes — and for admins, governance actions — land here."
        />
      ) : (
        active
          .filter((k) => byKind.get(k)?.length)
          .map((k) => (
            <section key={k}>
              <div className="mb-2 flex items-baseline gap-2">
                <span className="text-muted">{KIND_META[k].icon}</span>
                <h2 className="text-sm font-semibold text-fg">{KIND_META[k].label}</h2>
                <span className="text-xs text-muted">{KIND_META[k].blurb}</span>
                <span className="ml-auto text-xs text-muted">{byKind.get(k)!.length}</span>
              </div>
              <Panel className="p-0">
                <div className="divide-y divide-line-subtle">
                  {byKind.get(k)!.map((e, i) => (
                    <button
                      key={`${e.at}-${i}`}
                      type="button"
                      onClick={() => void navigate({ to: e.href })}
                      className="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors hover:bg-card"
                    >
                      <Avatar name={e.actor} className="mt-0.5 h-6 w-6 shrink-0 text-xs" />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-baseline gap-2">
                          <span className="truncate text-sm font-medium text-fg">{e.actor}</span>
                          <span className="shrink-0 text-xs text-muted">{e.context}</span>
                          {e.type && (
                            <Chip tone={WARN_TYPES.has(e.type) ? 'warn' : 'neutral'} className="shrink-0">
                              {e.type}
                            </Chip>
                          )}
                          <span className="ml-auto shrink-0 text-xs text-muted">{relativeTime(e.at)}</span>
                        </div>
                        <div className="truncate text-sm text-muted">{e.detail}</div>
                      </div>
                    </button>
                  ))}
                </div>
              </Panel>
            </section>
          ))
      )}
    </div>
  )
}

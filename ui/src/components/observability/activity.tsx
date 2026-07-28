import { useNavigate } from '@tanstack/react-router'
import { SkeletonRows } from '@/components/ui/skeleton'
import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Panel } from '@/components/ui/panel'
import { relativeTime } from '@/lib/fleet'


type Kind = 'ticket' | 'channel' | 'fleet'

interface ActivityEvent {
  at: string
  kind: Kind
  actor: string
  context: string
  detail: string
  href: string
}

const KIND_META: Record<Kind, { label: string; icon: string }> = {
  ticket: { label: 'Tickets', icon: '⧉' },
  channel: { label: 'Channels', icon: '⋕' },
  fleet: { label: 'Fleet', icon: '◍' },
}

function useActivity(kinds: Kind[]) {
  return useQuery({
    queryKey: ['activity', kinds.join(',')],
    queryFn: async (): Promise<ActivityEvent[]> => {
      const qs = kinds.length ? `?kinds=${kinds.join(',')}` : ''
      const r = await fetch(`/api/activity${qs}`)
      if (!r.ok) throw new Error('failed to load activity')
      return ((await r.json()) as { events: ActivityEvent[] }).events
    },
    refetchInterval: 30_000,
  })
}

// Everything that happened across the workspace — tickets, channels, agent
// config changes — merged into one stream, scoped to what this user can see.
export function AuditPanel() {
  const [kinds, setKinds] = useState<Kind[]>([])
  const { data: events = [], isLoading } = useActivity(kinds)
  const navigate = useNavigate()

  const toggle = (k: Kind) =>
    setKinds((prev) => (prev.includes(k) ? prev.filter((x) => x !== k) : [...prev, k]))

  return (
    <div>
      <div className="space-y-8">
        <div className="flex items-center gap-3">
          <div className="ml-auto flex gap-1.5">
            {(Object.keys(KIND_META) as Kind[]).map((k) => {
              const on = kinds.length === 0 || kinds.includes(k)
              return (
                <button
                  key={k}
                  type="button"
                  onClick={() => toggle(k)}
                  className={cn(
                    'rounded-full border px-3 py-1 text-xs transition-colors',
                    on ? 'border-accent text-fg' : 'border-line-subtle text-muted',
                  )}
                >
                  {KIND_META[k].icon} {KIND_META[k].label}
                </button>
              )
            })}
          </div>
        </div>

        {isLoading ? (
          <SkeletonRows rows={8} avatar />
        ) : events.length === 0 ? (
          <EmptyState
            icon="☰"
            title="Nothing yet"
            hint="Ticket updates, channel messages, and agent config changes land here."
          />
        ) : (
          <Panel className="p-0">
            <div className="divide-y divide-line-subtle">
              {events.map((e, i) => (
                <button
                  key={`${e.at}-${i}`}
                  type="button"
                  onClick={() => void navigate({ to: e.href })}
                  className="flex w-full items-start gap-3 px-6 py-3 text-left transition-colors hover:bg-card"
                >
                  <span className="mt-0.5 w-5 shrink-0 text-center text-muted">{KIND_META[e.kind].icon}</span>
                  <Avatar name={e.actor} className="mt-0.5 h-6 w-6 shrink-0 text-xs" />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline gap-2">
                      <span className="truncate text-sm font-medium text-fg">{e.actor}</span>
                      <span className="shrink-0 text-xs text-muted">{e.context}</span>
                      <span className="ml-auto shrink-0 text-xs text-muted">{relativeTime(e.at)}</span>
                    </div>
                    <div className="truncate text-sm text-muted">{e.detail}</div>
                  </div>
                </button>
              ))}
            </div>
          </Panel>
        )}
      </div>
    </div>
  )
}

import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Panel } from '@/components/ui/panel'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useMarkNotificationsRead, useNotifications, type Notification } from '@/lib/notifications'

// The notifications half of the Inbox surface: mentions (and more kinds as
// they land), newest first, mark-read on open. Lives at the top of `/` now —
// the old standalone /inbox page redirects here.
export function NotificationsPanel() {
  const navigate = useNavigate()
  const query = useNotifications()
  const { data } = query
  const markRead = useMarkNotificationsRead()
  const items = data?.notifications ?? []
  const unread = data?.unread ?? 0
  // Collapsed by default: the briefing above is the working surface — this is
  // the raw feed, one click away, with the unread count doing the talking.
  const [expanded, setExpanded] = useState(false)

  const open = (n: Notification) => {
    void markRead([n.id])
    if (n.href) void navigate({ to: n.href })
  }

  // A rejected read leaves `data` undefined FOR EVER, and `!data` was the only
  // thing standing in for it — so a 500 on /api/notifications shimmered a
  // skeleton at the top of the Inbox permanently, with no error text and no
  // way to retry. Broke, loading, and resolved-empty are three answers.
  if (query.isError && data === undefined)
    return (
      <Panel>
        {/* Inline density on purpose: collapsed, this panel is one row tall,
            and a full-height error block at the top of Home would shove the
            briefing down further than the bug ever did. */}
        <QueryError
          variant="inline"
          title="Could not load notifications"
          error={query.error}
          onRetry={() => void query.refetch()}
        />
      </Panel>
    )
  // In flight → hold a modest space at the top of the page (this panel leads
  // the column, so popping in late shoves EVERYTHING down). Resolved empty →
  // null: a quiet inbox takes no space.
  if (!data)
    return (
      <Panel>
        <Skeleton className="mb-3 h-3 w-28 rounded-full" />
        <SkeletonRows rows={2} />
      </Panel>
    )
  if (items.length === 0) return null

  return (
    <Panel>
      <div className={expanded ? 'mb-2 flex min-h-6 items-center gap-3' : 'flex min-h-6 items-center gap-3'}>
        <button type="button" onClick={() => setExpanded((v) => !v)} className="group flex min-w-0 items-center gap-2 text-left">
          {expanded ? <ChevronDown size={12} className="shrink-0 text-muted" /> : <ChevronRight size={12} className="shrink-0 text-muted" />}
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors group-hover:text-muted">
            Notifications
          </span>
          {unread > 0 && (
            <span className="font-mono text-[10px] font-medium tracking-[0.05em] text-accent">{unread}</span>
          )}
          {unread === 0 && (
            <span className="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">all read</span>
          )}
        </button>
        {expanded && unread > 0 && (
          <Button variant="outline" size="sm" className="ml-auto" onClick={() => void markRead()}>
            Mark all read
          </Button>
        )}
      </div>
      {/* Polled every 30s: a failed refresh keeps the last good feed on screen
          (stale beats blank) but must not pass an old unread count off as
          current. */}
      {query.isError && (
        <QueryError
          variant="inline"
          className="mt-2"
          title="Notifications may be out of date"
          error={query.error}
          onRetry={() => void query.refetch()}
        />
      )}
      {expanded && (
      <ul className="max-h-80 space-y-1 overflow-y-auto">
        {items.map((n) => (
          <li key={n.id}>
            <button
              type="button"
              onClick={() => open(n)}
              className={cn(
                'w-full rounded-md border border-transparent px-3 py-2.5 text-left transition-colors hover:bg-card2',
                !n.readAt && 'border-line bg-raised',
              )}
            >
              <div className="flex items-baseline gap-2">
                {!n.readAt && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />}
                <span className={cn('min-w-0 flex-1 truncate font-sans text-sm', n.readAt ? 'text-muted' : 'font-medium text-fg')}>
                  {n.title}
                </span>
                <span className="shrink-0 font-mono text-[11px] text-muted">{relativeTime(n.createdAt)}</span>
              </div>
              {n.body && <div className="mt-0.5 truncate pl-3.5 font-sans text-xs text-muted">{n.body}</div>}
            </button>
          </li>
        ))}
      </ul>
      )}
    </Panel>
  )
}

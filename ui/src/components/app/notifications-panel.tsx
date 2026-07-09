import { useNavigate } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { Panel } from '@/components/ui/panel'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useMarkNotificationsRead, useNotifications, type Notification } from '@/lib/notifications'

// The notifications half of the Inbox surface: mentions (and more kinds as
// they land), newest first, mark-read on open. Lives at the top of `/` now —
// the old standalone /inbox page redirects here.
export function NotificationsPanel() {
  const navigate = useNavigate()
  const { data } = useNotifications()
  const markRead = useMarkNotificationsRead()
  const items = data?.notifications ?? []
  const unread = data?.unread ?? 0

  const open = (n: Notification) => {
    void markRead([n.id])
    if (n.href) void navigate({ to: n.href })
  }

  if (items.length === 0) return null // a quiet inbox takes no space

  return (
    <Panel>
      <div className="mb-2 flex items-center gap-3">
        <span className="text-sm font-semibold text-fg">Notifications</span>
        {unread > 0 && <span className="text-xs text-muted">{unread} unread</span>}
        {unread > 0 && (
          <Button variant="outline" size="sm" className="ml-auto" onClick={() => void markRead()}>
            Mark all read
          </Button>
        )}
      </div>
      <ul className="max-h-80 space-y-1 overflow-y-auto">
        {items.map((n) => (
          <li key={n.id}>
            <button
              type="button"
              onClick={() => open(n)}
              className={cn(
                'w-full rounded-xl border border-transparent px-3 py-2.5 text-left transition-colors hover:bg-card',
                !n.readAt && 'border-line-subtle bg-card',
              )}
            >
              <div className="flex items-baseline gap-2">
                {!n.readAt && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />}
                <span className={cn('min-w-0 flex-1 truncate text-sm', n.readAt ? 'text-muted' : 'font-medium text-fg')}>
                  {n.title}
                </span>
                <span className="shrink-0 text-xs text-muted">{relativeTime(n.createdAt)}</span>
              </div>
              {n.body && <div className="mt-0.5 truncate pl-3.5 text-xs text-muted">{n.body}</div>}
            </button>
          </li>
        ))}
      </ul>
    </Panel>
  )
}

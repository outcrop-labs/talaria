import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useMarkNotificationsRead, useNotifications, type Notification } from '@/lib/notifications'

export const Route = createFileRoute('/_app/inbox')({
  component: InboxPage,
})

// The notification inbox — mentions land here (more kinds later).
function InboxPage() {
  const navigate = useNavigate()
  const { data } = useNotifications()
  const markRead = useMarkNotificationsRead()
  const items = data?.notifications ?? []
  const unread = data?.unread ?? 0

  const open = (n: Notification) => {
    void markRead([n.id])
    if (n.href) void navigate({ to: n.href })
  }

  return (
    <div className="mx-auto w-full max-w-2xl p-6">
      <div className="mb-4 flex items-center gap-3">
        <h1 className="mercury-text text-lg font-semibold">Inbox</h1>
        {unread > 0 && (
          <>
            <span className="text-xs text-muted">{unread} unread</span>
            <Button variant="outline" size="sm" className="ml-auto" onClick={() => void markRead()}>
              Mark all read
            </Button>
          </>
        )}
      </div>

      {items.length === 0 ? (
        <EmptyState icon="⌾" title="Nothing here yet" hint="@mentions from channels land in your inbox." />
      ) : (
        <ul className="space-y-1">
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
      )}
    </div>
  )
}

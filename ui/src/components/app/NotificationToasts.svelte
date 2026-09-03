<script lang="ts">
  import {
    browserNotifyEnabled,
    fireBrowserNotification,
    permissionState,
    shouldBrowserNotify,
  } from '@/lib/browser-notify'
  import { useNotifications, type Notification } from '@/lib/notifications'
  import { ArrivalTracker } from '@/lib/notify-arrivals'
  import { pushToast } from '@/lib/toast.svelte'
  import { onUserEvent } from '@/lib/user-events.svelte'

  // The shell's watcher: turns live notifications into toasts, and into OS
  // notifications when Talaria is not what the person is looking at. Renders
  // nothing. Mounted in AppLayout so it runs on every surface — the toast is
  // only honest if a mention that lands while you are on Boards reaches you
  // there, not just on Home where the feed lives.
  //
  // Liveness is the same two-legged shape as the Home feed: the firehose (one
  // shared EventSource per tab, user-events.svelte) refetches the moment a
  // row lands, and the query's own 30s poll (which this mount keeps alive
  // app-wide) is the floor for a dropped SSE or a sleeping laptop. The event
  // carries no content on purpose — the ordinary read path stays the only
  // reader.
  const query = useNotifications()
  const tracker = new ArrivalTracker<Notification>()

  $effect(() =>
    onUserEvent((event) => {
      if (event.type !== 'notification') return
      void query.refetch()
    }),
  )

  // Arrivals: rows this tab has never seen. The first page seeds the tracker
  // silently (a reload must not replay the inbox as toasts); only what lands
  // after that toasts. Focus is read HERE, at arrival time, not tracked — the
  // question is what the person is looking at now, and this effect runs at
  // exactly that moment.
  $effect(() => {
    const rows = query.data?.notifications
    if (!rows) return
    for (const n of tracker.arrive(rows)) {
      pushToast({ title: n.title, body: n.body || undefined, href: n.href || undefined })
      if (
        shouldBrowserNotify({
          focused: document.hasFocus(),
          visible: document.visibilityState === 'visible',
          permission: permissionState(),
          enabled: browserNotifyEnabled(),
        })
      ) {
        // The notification id doubles as the OS tag: two open tabs each see
        // the arrival, and the OS shows it once.
        fireBrowserNotification({ title: n.title, body: n.body || undefined, tag: n.id, href: n.href || undefined })
      }
    }
  })
</script>

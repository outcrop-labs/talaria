- **Toasts, and a tap on the shoulder when you're elsewhere.** New
  notifications (a mention, a DM, a share, an approval) now surface as
  in-app toasts on every surface: a small stack bottom-right, click to go
  there, six and a half seconds and gone. A watcher in the app shell rides
  the same liveness the Home feed uses (the SSE firehose plus the 30s poll
  as the floor), so a mention that lands while you're on Boards reaches you
  there. The first read is the baseline: reloading never replays the inbox
  as a burst of toasts, and past four stacked the oldest quietly goes. The
  browser half is deliberate: an OS notification fires ONLY when Talaria is
  not what you're looking at (another tab, another window, minimized) —
  while you're looking at it, the in-app toast already said it. Permission
  is asked from your click, never on load: Settings → Notifications gains a
  Desktop notifications row (On/Off, with the blocked case explained), and
  a local off switch stands in for the grant browsers won't let a page hand
  back. Two open tabs each see the arrival; the OS shows it once (the
  notification id doubles as the OS tag). This covers the open-but-
  background case, which the plain Notification API does from a tab;
  notifying a fully closed browser is Web Push and stays unmade. Verified
  live: a row landing while focused toasts with no OS notification, the
  toast click routes to its target, the same arrival with focus elsewhere
  fires the OS notification with the right tag (headless Chromium denies
  real notifications and reports every page focused, so that leg ran
  against browser stubs plus unit tests of the gate), and a reload toasts
  nothing. 11 new tests; full suite green (2580).

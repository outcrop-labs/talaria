# Notifications

How a notification gets from the moment something happens to the person it is for: the
row, the live stream, the toast, the OS banner, Web Push, the desktop shell, the click.
It also covers the contracts the browser runtime and the desktop shell share, and what to
check when an alert does not arrive.

The desktop shell's own half (its OS backends, tray and badge) is
[`DESKTOP.md`](./DESKTOP.md).

## The paths, end to end

```
 a producer (mention, DM, assignment, approval, …)
        │
        ▼
 talaria_notify::add_notification            ← the ONE writer (api/crates/talaria-notify)
   ├─ the row (notifications), unread, href validated to a same-origin path
   ├─ SSE  {type:"notification", notificationId}   on user:<id>   → every open tab
   ├─ the brief nudge
   ├─ Web Push (route ≠ email)                → every push_subscriptions row → service worker
   └─ email (route email|both, instance switch on)
```

- **An open tab.** `/api/me/events` (user-events.svelte.ts) invalidates the inbox query; the
  arrival record (notify-arrivals.ts) says which rows this browser has not surfaced; the
  decider (notify-decide.ts) makes one call per browser: a toast in the tab the person is
  looking at, else one OS banner from the leader tab, else nothing. When this browser cannot
  post a banner (not granted, a plain-http origin, or switched off here), the visible tab
  touched last toasts instead, so a Talaria window on another monitor still shows it. The
  inbox query also polls every 30 s, hidden tabs included.
- **Every Talaria tab closed.** Web Push: talaria-push encrypts the row (RFC 8291) under the
  instance VAPID identity (RFC 8292) and posts it to every subscription; the service worker
  (`ui/src/sw.ts`) shows it unless the person is looking at Talaria or the leader tab
  already showed it. The browser itself must be running: on Linux and Windows closing its
  last window quits it, and Settings says so on those systems.
- **The desktop shell.** The page posts through `shellShowNotification` instead of the
  browser's API; see [`DESKTOP.md`](./DESKTOP.md).
- **The click.** Every path (toast, OS banner via the worker, the shell's event, the
  `new Notification` fallback) goes through `notify-open.ts`: the href is checked, the app
  routes in place, and the row is marked read. A notification with no openable link (an
  empty href, or one the rule refuses) leaves the person on the page they are on; only the
  worker, with no window to focus, opens `/`. Only a row id is ever marked read: the
  summary's `talaria-summary` and the confirmation banner's empty id are not rows.

## What a person sees and sets

The center itself (the bell, the page, the settings, the status panel) is
[`notifications/center.md`](./notifications/center.md); this is the short version.

- **The bell** has two tabs. **For you** is what needs you or was addressed to you
  (approvals, escalations, blocked agents, your turn, mentions, assignments, shares, direct
  messages, conversations you are part of). **All** adds what you follow (ticket moves,
  finished research, channel activity) and the workspace's health for admins. The badge
  counts unread For you only. Rows about one thing (a ticket, a channel) sit under one
  header ("TAL-112 · 3 updates"), a row that stands for several events says so ("3
  messages", "Moved 4 times"), and each row says why it reached you ("Mentioned you").
  Clicking a row opens it in place and marks it read. Each row can be marked Done, which
  also marks it read, or read and unread; the header pauses notifications and marks all
  read; View all opens the full page.
- **`/notifications`** is the same inbox on a page: For you, All and Done; unread only, by
  category, and by what happened; as many rows as you scroll; and the keyboard (`j`/`k`
  move, `Enter` opens, `e` Done, `u` read or unread, `x` selects, `Shift+E` Done everything
  shown or selected, `?` lists them).
- **Settings → Notifications** is a matrix: one row per kind of notification, grouped as
  Needs you, Direct, Following and Workspace, and one column per channel. **In-app** is
  always on: every notification is kept in your inbox. **Desktop** is a banner on this
  computer from a Talaria tab or the desktop app. **Push** is a banner from your browser
  with every Talaria tab closed. **Email** is a missed-activity email, sent as often as you
  choose (every 15 minutes, hourly, daily or never), and disabled, with the reason, while
  the instance's email switch is off. The daily digest keeps its own switch.
- **Do Not Disturb.** Pause from the account menu or the bell for 30 minutes, an hour, two
  hours, until 9:00 tomorrow in your time zone, until you resume, or until a time you pick;
  and set working hours, outside which notifications arrive quietly. Needs you comes through
  either way unless you turn that off. The bell shows "Paused until …" with Resume.
- **The status panel** (Settings → Notifications) says, live, whether this browser can
  show alerts and why not, which browsers are registered for push and what the push service
  last said to each, a test per device, which of your devices alerts go to right now, the
  desktop app's own notification service when you are in it, and the pause in force.

## Asking for permission

The browser's prompt is a one-shot: dismissed a few times, Chrome embargoes the origin, and
a Block is near-permanent. Talaria never prompts on load.

- **The card** (`NotifyPrompt.svelte`, rules in `notify-prompt.ts`) shows only when the
  permission is `default`, the context is secure, this is not the shell, the page has been
  open at least 10 s and touched, and a reason just fired: an arrival was toasted in this
  focused tab, or the person came back to the tab (into view, or focused) after one landed
  while they were not looking at it.
- **"Not now"** is remembered on the account (`users.notify_prefs.prompt`, written by
  `PATCH /api/notifications {prompt:{dismissed:true}}`): 7 days after the first, 30 after
  the second, never after the third. A grant ends it. An answer also holds for the page's
  life, whatever a refetch or a failed write says.
- **The bell's row** stays: "Turn on desktop alerts", "Desktop alerts are blocked in
  \<Browser\>" (with the steps), or, on a plain-http dev stack, why it cannot work.
- **Settings → Notifications → Status and troubleshooting → This browser** shows the same
  state live, with the one control that state allows, whether this browser is registered for
  alerts with every Talaria tab closed (and whether the server has it on file), and, on
  Linux and Windows, that closing the browser's last window quits it (and stops them).
- **Turn on** calls `Notification.requestPermission()` synchronously inside the click. On a
  grant it files the push subscription and shows a local "Desktop alerts are on" banner
  through the same registration a push uses.

## HTTPS

Notifications, push and service workers exist only in a secure context. Every real Talaria
instance is reached over HTTPS (behind Cloudflare, or any TLS proxy), so this only affects a
development stack on plain http reached by LAN IP or hostname (`http://localhost` is itself
secure). There, the bell and Settings say so and name the origin; there is no toggle to
press.

## The live stream behind an edge

Every real instance sits behind an edge that holds idle connections for about 100 s and
turns an origin restart into a 502 or 524.

- The api pings every stream (`: ping`) after 20 s idle and opens each with `retry: 3000`.
- Every stream carries `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`;
  the app's proxy passes both through.
- Clients reopen a CLOSED stream with backoff (1 s, 2 s, 4 s … 30 s, ±20 %), at once on
  `online` or when the tab becomes visible, and stop after five failures if the session is
  gone (`lib/sse.ts`).
- On every reopen the user stream refetches every query it drives (the stream carries no
  replay).
- A subscriber is dropped the moment its client leaves, not on the topic's next publish.
- On SIGTERM the api ends every open stream (`talaria_realtime::end_streams`) before its
  graceful drain, so the drain is not held open by connected browsers; each reconnects after
  its `retry:` to the instance that replaces it.

## One banner per notification per browser

- **The leader.** Every tab asks for the Web Lock `talaria-notify-leader`; the holder posts
  OS banners. When it closes, the next tab gets the lock at once.
- **The registry.** Tabs announce `{tabId, visible, focused, lastInputAt, at}` on
  `BroadcastChannel('talaria-notify')` on every focus or visibility change and every 10 s;
  entries older than 15 s are ignored. A tab that toasts says `claimed`, and the leader waits
  300 ms for a claim before it posts.
- **One namespace.** Pages post through `registration.showNotification` whenever a
  registration exists, the tag being the row id, so a page banner and a push of the same row
  are one notification. `new Notification` is the fallback without a worker.
- **The bannered store.** IndexedDB `talaria-notify` / `bannered`, keyed by row id with the
  row version (its `createdAt`), written by whoever shows a banner and read by both before
  showing.
- **The worker** skips a push when a window is focused and visible, or when that version is
  already bannered or on screen. On WebKit it never skips, because Safari revokes
  subscriptions after silent pushes.
- **Banners off.** With no banner to post, the tab the person looks at toasts; else the
  visible tab with the latest input (ties to the lowest tab id). Each tab knows its own input
  exactly and the others' only as of their last announcement, so the true latest tab always
  picks itself: a race can show an arrival twice, never zero times.
- **Back/forward cache.** A page restored from it starts a new runtime (lock, registry) on
  `pageshow`.
- Without Web Locks or BroadcastChannel every tab decides alone, and the tag plus the
  bannered store still collapse duplicates.

## Catch-up instead of absorption

A browser with no record for this person baselines silently (a fresh profile must not
replay the inbox). Every later read, including the first after a reload, a reopen or a
deploy, surfaces what landed since. Up to three arrivals show one by one, oldest first;
more than three show as one summary ("5 new — …", tag `talaria-summary`) whose click opens
the bell. In the desktop shell the summary banner carries the page it was posted from
(`shellHref` in `browser-notify.ts`): the shell dispatches `talaria:notification-open` only
for a banner that names a path (I-1), and the page's listener opens the bell. Rows read
elsewhere are never surfaced, and neither is a row version this browser already showed as a
banner (the worker's, while every tab was closed), so the summary never stacks on top of the
worker's per-row banners.

## Push lifecycle

- **Boot.** A browser whose permission is granted (and not switched off in this browser)
  files its subscription at every boot and when the permission turns granted, with no click.
  A subscription bound to an old instance key is unsubscribed and replaced.
- **Subscribe.** `POST /api/push/subscribe` stores the hex SHA-256 of the session that filed
  it (`session_hash`); the session id itself is never stored.
- **Sign-out.** The client unsubscribes (bounded at 2 s) and closes every banner the origin
  still has up, the OS notification centre included; `POST /api/auth/logout` deletes every
  row with that session's hash. Other sessions' rows stay.
- **Delivery.** Devices are posted four at a time, at most 32 in flight per process, each
  POST bounded (5 s to connect, 15 s in all). The plaintext is held under 3,000 bytes; a 413
  gets one minimal resend. 429, 5xx and transport failures are retried at most twice
  (`Retry-After` up to 60 s, else 2 s then 8 s).
- **Row tending.** Every attempt writes `last_status`, `last_error`, `last_attempt_at`. A 2xx
  resets `failures`. 404 and 410 prune at once. 401 and 403 count, and the third failure in a
  row ending in one prunes. A 403 naming the instance's own token (Apple's `BadJwtToken`
  family) is logged as an instance fault and not counted.
- **VAPID subject.** `AUTH_PUBLIC_URL` when it is https, else the verified instance domain,
  else a mailto placeholder.
- **Dev safety.** `bun talaria worktree` and `talaria box seed` copy no `push_subscriptions`
  rows and drop the copied `push_vapid` key, so a dev stack cannot push to the real browsers
  of the database it was seeded from.

## Server rules

- **Href.** A row stores only a same-origin path: a leading `/`, not `//`, no backslash, no
  control characters, no `.` or `..` path segment in any `%2e` spelling, at most 2,048
  bytes. Anything else is stored as `''` (logged once per kind). The page
  (`notify-open.ts`) and the worker (`sw.ts`) apply the same rule before they open, and
  also refuse a path that would leave the origin when parsed again: `/..//evil.example`
  resolves to the path `//evil.example`, which a router or `navigate()` reads as another
  host.
- **DMs.** One unread `dm` row per conversation. A new message refreshes it in place (same
  id, the newest body, `created_at = now()`), publishes it, and pushes with `renotify`; it
  sends no new mail. Reading the channel to its newest message clears the row (and any
  mention pointing there). An open conversation reads only while the person is looking at
  it, shown and focused (`ui/src/lib/looking.ts`, the rule the toast decision uses too): a
  background tab, a minimized window or an instance behind the desktop shell's launcher
  leaves new messages unread, so their DM or mention still alerts, and the read lands the
  moment the person looks.
- **Mentions on edit.** A ticket description edit notifies only the people the edit adds.
- **Assignment.** A person assigned by a patch gets `task-assigned` only; the same patch's
  status move goes to every other watcher.
- **Read-sync.** Any mark-read (the bell, a toast, a banner click, a channel read) publishes
  `{type:"notifications", unread:N}`, and every tab's bell drops at once.

## Contracts with the desktop shell

These are the seams the browser runtime and the shell share. Each side works when the other
is older.

| | Contract |
|---|---|
| **I-1** | The shell's banner click dispatches a cancelable `talaria:notification-open` `CustomEvent` with `detail {href, id}` in the instance page. The page cancels it and opens the notification in place. With no listener the shell falls back to `location.assign(href)`. |
| **I-2** | The page passes `{title, body, tag, href, kind, renotify}` to `shellShowNotification` as a variable; a shell that does not know `kind` or `renotify` ignores them. |
| **I-3** | The arrival record, `localStorage['talaria.notify.arrivals.v1']`: `{ "<userId>": { "hw": ISO, "seen": { "<rowId>": ISO, … ≤ 300 }, "at": ms } }`. A row is an arrival when it is unread, newer than `hw − 120 s`, and unseen or newer than its recorded `createdAt` (a re-alert). The shell's suspend handoff reads and merges it. |
| **I-4** | `browserNotifyEnabled()` keeps its name and meaning. |
| **I-5** | `useNotifications()` keeps its name and payload (additions are optional fields: `prompt`). |
| **I-6** | The soft ask and the bell row never show in the shell; Settings → Notifications points to Settings → Desktop. |
| **I-7** | The row JSON the shell parses is unchanged: `notifications[].{id,kind,title,body,href,readAt,createdAt}` and top-level `unread`. |
| **I-8** | A known id with a later `createdAt` is a re-alert: the same tag, `renotify`. |
| **I-9** | `{type:"notifications","unread":N}` on `user:<id>` after any mark-read; consumers that do not know it ignore it. |

## Wave 2

The second round of the notification work builds on one contract and splits into six
streams. Each has its own doc; the shared interfaces (I-10 … I-24), the schema and its
measured cost, and the wire changes are in [the contract](./notifications/contract.md).

- [The contract](./notifications/contract.md): the seq cursor, the row and event wire v2,
  the class registry, prefs v2, DND, presence, the ack, the devices list.
- [Delivery engine](./notifications/delivery.md): what is delivered where and when.
- [The live stream and the inbox API](./notifications/live-and-inbox.md).
- [Producers](./notifications/producers.md): coalescing, participants, channel levels.
- [The web runtime](./notifications/web-runtime.md).
- [The notification center](./notifications/center.md): the bell, the matrix, DND, the
  status panel.

## Troubleshooting

Start at **Settings → Notifications → Status and troubleshooting**: it checks each of the
things below live and says which one is in the way.

- **Nothing arrives anywhere.** The Pause section says whether you paused, or are outside
  your working hours; the bell shows "Paused until …" too.
- **One device gets nothing.** Your devices lists every browser registered for push. Send
  test pushes through the same path a notification takes and shows what the push service
  answered (5 tests per 10 minutes). A device missing from the list is not registered: open
  Talaria in it and turn desktop alerts on. Remove retires one you no longer use.
- **Alerts go to the wrong device.** Live connection says which of your devices alerts go
  to now: the one you are looking at, else one you used in the last two minutes; with
  neither, every registered browser gets the push.
- **The bell says alerts are blocked.** Follow its steps: the browser's switch for this site
  *and* the OS's switch for the browser. The page notices the change with no reload.
- **Alerts work in a tab but not when the browser is closed.** The status panel's This
  browser section says whether this browser is subscribed and whether the server has it on
  file. Chrome and Edge receive push only while the browser
  process runs: on Windows and Linux, closing the last window quits it (unless "continue
  running background apps" is on); on macOS, closing the windows leaves it running.
- **Nothing pops, but the bell counts.** Do Not Disturb and Focus modes drop banners
  silently, on every OS.
- **Safari shows a banner while Talaria is in front.** Expected for now: Safari revokes a
  subscription after pushes that show nothing, so its worker never skips. Presence-gated push
  on the server removes it (wave 2).
- **A dev stack on http.** Expected: see HTTPS above.
- **A push row keeps failing.** `select endpoint, failures, last_status, last_error,
  last_attempt_at from push_subscriptions where user_id = …` shows the last answer per
  device.

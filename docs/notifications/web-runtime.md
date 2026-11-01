# The web runtime

The page's notification runtime (W2-R): one event stream per browser, topic
streams that sleep while hidden, a lossless catch-up by `seq`, the decision of
which device and which tab alerts, presence heartbeats and the ack, banners
and toasts closed when a row is read anywhere, and the ambient unread signals.
It is client code only, under `ui/src/lib/` and `ui/src/components/app/`. The
interfaces it builds on are in [the contract](./contract.md); the user-facing
story is in [`../NOTIFICATIONS.md`](../NOTIFICATIONS.md).

Every behaviour below is gated on what the api says it serves (`features` in
`GET /api/notifications`, I-23). Against an api that lacks a feature, the page
behaves as wave 1 did on that axis.

## One stream per browser (`notify-hub.ts`, `user-events.svelte.ts`)

`/api/me/events` used to be one EventSource per tab: five tabs were five
sockets and five Redis subscribers, and on a plain-http dev origin five of the
six connections a browser allows one host (BT-3).

- The tab that holds the leader lock (`notify-leader.ts`, the Web Lock
  `talaria-notify-leader`) is the **hub**. It alone opens the stream and posts
  every frame on the `talaria-notify` BroadcastChannel as `hub:frame`, every
  open as `hub:open`, and its connection state as `hub:state`.
- Every other tab is a **follower**: it says `hub:hello`, the hub answers with
  its state, and it hears the frames on the channel. A subscriber
  (`onUserEvent`, `useUserEvents`, `onUserStreamOpen`) cannot tell which it
  is.
- When the hub tab goes away (closed, crashed, discarded, or put in the
  back/forward cache) the lock passes to the next tab at once; that tab opens
  the stream itself and says `hub:open`, and every tab refetches what the
  stream drives (a reopen).
- A follower that hears from no hub within 2.5 s (a tab of an older build
  holds the lock during a deploy) opens its own stream, as wave 1 did, until a
  hub speaks up or it takes the lock itself. Without Web Locks or
  BroadcastChannel, every tab opens its own.
- The stream opens with `?since=<the arrival record's cursor>`, read at every
  reconnect, so an api that replays (I-19) sends what the gap dropped.
- A page restored from the back/forward cache starts a new runtime; a frozen
  page's `resume` refetches as after a reopen.
- A `session` frame (`state: "ended"`: the session behind the stream was
  signed out or expired) closes the tab's stream at once (reopening it would
  only collect 401s) and asks for the session until the read answers: the
  session gate (AppLayout) shows the sign-in screen on `{user: null}`, and a
  failed read is a backend blip, not an answer, so it is asked again on the
  door's backoff (1 s, 2 s, 4 s … 30 s) instead of leaving the signed-in page
  up. If the answer is "still signed in", the runtime starts again. The
  leader hands the frame to the other tabs before it closes, so every tab
  does this. When the door's own probe (after five failed reconnects) finds
  the person signed out — the frame was lost, or an older api sends none —
  it hands on the same frame.

The tab registry's announcements now also carry the tab's presence client id,
whether it leads, and whether it runs a hub.

## Topic streams sleep while hidden (`sse.ts`)

A board's, a channel's or a run's stream in a tab nobody can see is a socket
and a Redis subscriber spent on nothing. `openStream(url, fn, {pauseWhenHidden:
true, onResume})` closes the stream after the tab has been hidden for 30 s.
When the tab is shown again, `onResume` runs first (refetch what the stream
would have said, so it shows at once), then the stream reopens, and its
`onOpen` says `resumed`; the callers read once more then, so a change landing
between the first read and the subscription is not left for the next event.

- Boards refetch their cards; channels their messages and the channel; the
  work-watch pane reads the run's own record (`/api/tasks/{id}/work-sessions`,
  through the query cache, so the work log shows the same read) before it
  reopens, so a run that finished while the tab was hidden shows its
  end (C31). The run stream also reconnects after a 502 now, with the same
  read on reopen.
- A topic stream opened in a tab that is already hidden (a tab opened in the
  background, or a view re-created while its tab is out of sight) starts
  paused: no socket until the tab is shown.
- On an HTTP/1.x page (a dev stack; real instances are HTTP/2 at the edge) the
  wait is 5 s instead of 30, so the six-socket budget frees up as fast as the
  person moves between tabs. The page's protocol is the navigation entry's
  `nextHopProtocol`; a page from the HTTP cache reports it empty, so then the
  first same-origin resource that went to the network says it (another
  origin's, a font CDN on h2, says nothing about this host), and failing both
  a plain-http page is HTTP/1.1.
- The person's own `/api/me/events` never pauses: it is what alerts. The
  work-watch transcript tail (a fetch stream) is unchanged: a reopened tail
  replays the turn in flight, and its parent (`RunDetailModal`) keeps the
  frames it was handed, so pausing it needs that parent to drop them first.
- `url` may be a getter, read at every connect, and `onState` reports
  `connecting`, `open`, `reconnecting`, `paused` and `closed`.

## The arrival record v2 (`notify-arrivals.ts`, I-20)

The record keeps its key and wave 1's `hw`/`seen`/`at`, and gains `v: 2` and
`seq`, the person's notify seq this browser has read through.

- With `since` in `features`, the tracker reads `GET /api/notifications?since=
  <cursor>` on every `notification` frame, every `resync`, every stream
  (re)open and every poll whose snapshot `seq` is past the cursor. The reads
  are coalesced: a burst of 60 frames is two reads, and a `notification`
  frame whose seq the cursor already covers (a replay sends one per row past
  the stream's cursor) asks for none. Pages of 200 are followed until `more`
  is false (at most 10 per catch-up; the next catch-up reads on).
- A row with a seq, under a record with a cursor, is an arrival iff it is
  unread, its seq is past the cursor, and it is unseen or its `createdAt`
  moved later (a re-alert). There is no margin. A Quiet coalesce takes a new
  seq but keeps its `createdAt`, so it is not news.
- The cursor moves to the snapshot `seq` of a read whose `more` is false (or,
  after the page budget, to the last row read: a page is complete up to its
  last row, I-10). Two tabs' records merge to the furthest cursor.
- A browser with no record for the person baselines silently at the list's
  snapshot `seq`. A wave-1 record (no cursor) reads `?since=0` once, and wave
  1's rule (high-water minus 120 s, plus `seen`) decides those rows, so nothing
  it already surfaced comes back.
- A row without a seq (filed before the contract), and every row against an
  api without `since`, keep wave 1's rule over the newest 50.
- **The server's seq can go back** (a database restored from a backup, a dev
  stack reset): a `?since` read then answers no rows and a `seq` below the
  cursor, and so do the stream's `resync` and the list's snapshot. Every row
  filed since has a seq at or below the old cursor, so the tracker
  **rebases**: it takes the server's seq, judges the newest page by wave 1's
  rule (what was filed since the restore surfaces; nothing older replays),
  and stamps the record `rebased` (ms epoch). A `resync` or a list snapshot
  below the cursor asks for that read. In a merge the record rebased last
  holds the cursor, so another tab's pre-restore cursor cannot put the old
  seq back; that tab rebases on its own next read.
- More than three arrivals at once are one summary under `SUMMARY_TAG`
  (`talaria-summary`, the same tag the shell's watcher uses, I-21).

## The decider (`notify-decide.ts`)

Each tab decides each arrival. The first rule that answers wins:

1. The row is read: nothing.
2. Its `alert` is `feed` (I-15): nothing. It is recorded and counted in the
   bell, never toasted or bannered.
3. Do Not Disturb is active and the row's class does not break through (I-14;
   `allowNeedsYou: false` holds Needs-you rows too): nothing. "Active" is as
   of now, not as of the list's read: the list can be a poll old (a minute in
   a throttled hidden page, or a `?since` read that beat the list's refetch),
   so a pause whose `until` has passed since holds nothing (`dndHolds`). A
   pause that has started since is the server's to hold: it files the rows
   written during it as `feed`.
4. **Viewing**: this tab is looking (shown, focused, and touched in the last
   60 s) at the row's own page, and the row is Direct or Following: nothing,
   and the row is marked read (EB-13). A hidden tab never qualifies (C8).
5. This tab is focused and visible: the in-app toast.
6. Another fresh tab of this browser is focused and visible: nothing (it
   toasts).
7. **Cross-device**: the presence winner (I-17) is a client of another device:
   nothing. The winner is compared with this tab's client id and every fresh
   tab's announced one; a winner among them is this device. Inside the
   desktop shell, so is the shell's own watcher for this instance,
   `desk-<local instance id>` (the registry entry's `id`, found as the
   switcher finds the current instance): it beats while the page is
   suspended, and after a restore its last `active` can be the winner for up
   to 75 s while the page's first state is `idle`.
8. Wave 1's tail: with banners off, the visible tab touched last toasts; else
   the leader posts the OS banner after the claim window.

With no presence (an api without `presence-store`, or no winner: the person is
idle everywhere), rule 7 does not apply and the tail is wave 1's per-browser
rule. The matrix is `notify-decide.test.ts`.

**Visit marks read.** When the route changes, or the tab becomes looking
again, the unread Direct and Following rows whose href path equals the current
path are marked read (by id). Needs-you rows clear when they are resolved,
never on a glance.

## Presence and the ack (`notify-presence.ts`, `presence.svelte.ts`, I-17, I-18)

The heartbeat AppLayout mounts now sends a body on `PUT /api/me/presence`:
`{clientId, kind, state, osIdleMs?}`.

- `clientId` is per tab (sessionStorage `talaria.notify.client`). A
  duplicated tab starts with a copy of its original's sessionStorage, so the
  two would report as one client and overwrite each other's state. The tab
  registry spots the twin (both announce the same id); the one with the
  greater tab id takes a fresh id, announces it, and beats under it at once.
- `kind` is `desktop` inside the shell, with `osIdleMs` from
  `shellOsIdleMs()` (I-22); without it, page signals only.
- `state`: **looking** = shown, focused, input within 60 s; **active** = shown
  with input within 120 s (inside the shell: OS input within 120 s, window or
  no window); **idle** = shown, untouched; **hidden** = not shown. Input is a
  pointer, a key, the wheel or a touch (pointer moves sampled once a second;
  `looking.ts`). A page that opens shown and focused counts its load as input.
- It is sent at once, on every change (at most one beat per 5 s), and every
  30 s. Hidden tabs keep beating as `hidden`. A tab that goes away (closed,
  navigated off, into the back/forward cache) sends a last `hidden` beat with
  `keepalive`, so the winner moves to the person's other device at once rather
  than when the entry ages out (75 s). The old no-body ping is gone;
  the route still sets the directory's online key (for a web tab unless it is
  hidden, per the contract's `marks_online`).
- The winner comes from `presence` frames, from each heartbeat's answer
  (`{ok, looking, active}`), and from `GET /api/me/presence` at start. It
  feeds the decider and `notify-status.setPresence`.
- **The ack.** A toast shown to a looking person, and an OS banner the leader
  posted (the page or the shell path), are acked on `POST
  /api/notifications/ack`, batched for 500 ms, each id once (with `ack` in
  `features`). The deferred push (W2-E) skips acked rows.

## Read-sync (`notify-readsync.ts`, I-16)

On a `notifications` event:

- `readIds`: the OS banners whose tag is one of them are closed
  (`registration.getNotifications()`), their toasts are dismissed (toasts now
  carry the row id as their key), and inside the shell
  `shellCloseNotifications(readIds)` asks it to close its own. A catch-up
  summary whose rows are all read goes too. The shell takes at most 100 tags
  a call, so longer lists go in chunks of 100, the summary tag first.
- `all`: every banner and every notification toast; in the shell, the tags
  this browser bannered in the last 48 hours plus the summary tag.
- `resync`: refetch, then close every banner whose row is no longer unread.
- A counts-only event closes nothing.

Every tab runs it (the hub hands every tab the frame); closing a banner that
is already gone is a no-op.

## Ambient signals (`notify-ambient.ts`, BT-9, EB-11)

All on the For-you count (`unreadForYou`, O-10), from the list and from each
read-sync event:

- the title gets a `(n) ` prefix on every route. The title is TabTitle's and
  the routes', so a MutationObserver on `<head>` puts the prefix back whenever
  it is rewritten, and a prefix of ours is replaced, never stacked;
- the favicon gets a dot (a canvas over the page's own icon), and the original
  icon is restored at zero;
- `navigator.setAppBadge(n)` where the browser has it (an installed web app's
  dock or taskbar icon), `setAppBadge(0)` at zero. The shell keeps its own
  badge (DesktopSwitcher).

Everything goes back to plain when the runtime stops (sign-out).

`ui/public/manifest.webmanifest` (linked from `ui/index.html`) names the app,
its icons (`/icons/notify-192.png`, `/favicon.svg`), `start_url /` and
`display: standalone` (WP-14): what an installed Talaria is drawn with, and
what Safari needs before a Home Screen web app may receive push.

## The service worker's pushes (`sw.ts`)

A push whose `createdAt` is OLDER than the version already shown under its
tag (the bannered store, which the leader tab writes too, or the copy still
on screen) is dropped on every engine: not shown, and not replacing the newer
copy. Two pushes of one row filed in quick succession are separate sends and
can arrive out of order, and a push service that replaces by `Topic` may keep
only the older. Pushes are handled one at a time, so two arriving together
cannot both pass the check. On WebKit a dropped push is one that shows
nothing, which Safari counts against the subscription; it takes two sends of
one row crossing in flight.

## Diagnostics

`window.__talariaNotify` (read-only) answers the tab id, the client id,
`isLeader()`, `hubRole()`, `presence()`, the tracker's `cursor()`, and the
last 50 decisions with their reasons (`recent()`). The status panel reads
`notify-status` (W2-C).
